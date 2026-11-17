use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

use crate::telemetry::{Device, Issue, Status, unix_ms};

type Error = Box<dyn std::error::Error>;

#[derive(Deserialize, Serialize)]
struct FrameSample {
    app: String,
    swap_chain: String,
    pid: u32,
    intervals: usize,
    fps: f64,
    frame_ms: f64,
    low_fps: Option<f64>,
    captured_at_unix_ms: u64,
}

static FOREGROUND_SAMPLE: Mutex<Option<FrameSample>> = Mutex::new(None);
const PRESENTMON_SHA256: &str = "b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af";

fn capture(presentmon: &Path, pid: u32) -> Result<Vec<FrameSample>, Error> {
    let session = format!("WallpaperDashboardProbe-{}", std::process::id());
    let output = Command::new(presentmon)
        .args(["--process_id", &pid.to_string(), "--session_name", &session])
        .args([
            "--timed",
            "5",
            "--terminate_after_timed",
            "--output_stdout",
            "--no_console_stats",
            "--no_track_input",
            "--v1_metrics",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "PresentMon failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let mut csv = csv::Reader::from_reader(output.stdout.as_slice());
    let headers = csv.headers()?;
    let column = |name: &str| {
        headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("PresentMon CSV has no {name} column"))
    };
    let application = column("Application")?;
    let chain = column("SwapChainAddress")?;
    let frametime = column("msBetweenPresents")?;

    let mut streams = BTreeMap::<(String, String), Vec<f64>>::new();
    for record in csv.records() {
        let record = record?;
        let Ok(ms) = record[frametime].parse::<f64>() else {
            continue;
        };
        if ms > 0.0 && ms.is_finite() {
            streams
                .entry((record[application].to_owned(), record[chain].to_owned()))
                .or_default()
                .push(ms);
        }
    }
    if streams.is_empty() {
        return Err(format!("no frame intervals captured for PID {pid}").into());
    }

    let captured_at_unix_ms = unix_ms();
    Ok(streams
        .into_iter()
        .map(|((app, swap_chain), mut times)| {
            let intervals = times.len();
            let frame_ms = times.iter().sum::<f64>() / intervals as f64;
            let low_fps = (intervals >= 100).then(|| {
                times.sort_unstable_by(f64::total_cmp);
                let tail = &times[intervals - intervals.div_ceil(100)..];
                1000.0 / (tail.iter().sum::<f64>() / tail.len() as f64)
            });
            FrameSample {
                app,
                swap_chain,
                pid,
                intervals,
                fps: 1000.0 / frame_ms,
                frame_ms,
                low_fps,
                captured_at_unix_ms,
            }
        })
        .collect())
}

pub fn report(presentmon: &Path, pid: u32) -> Result<(), Error> {
    for sample in capture(presentmon, pid)? {
        println!(
            "  {} swap chain {}: {:.1} presented FPS, {:.2} ms average, {} intervals",
            sample.app, sample.swap_chain, sample.fps, sample.frame_ms, sample.intervals
        );
        if let Some(low) = sample.low_fps {
            println!("    1% low: {low:.1} FPS (mean of slowest 1%)");
        } else {
            println!("    1% low: unavailable (fewer than 100 intervals)");
        }
    }
    Ok(())
}

fn feed_path() -> io::Result<PathBuf> {
    let root =
        env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA unavailable"))?;
    Ok(PathBuf::from(root)
        .join("WallpaperDashboard")
        .join("fps.json"))
}

fn target_start(pid: u32) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the output values are writable, and the process handle is closed here.
    let ok = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let result = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(process);
        result != 0
    };
    ok.then_some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

pub fn track(presentmon: &Path, pid: u32) -> Result<(), Error> {
    let file = feed_path()?;
    let folder = file.parent().ok_or("invalid FPS feed path")?;
    fs::create_dir_all(folder)?;
    let started = target_start(pid).ok_or("target process is not running")?;
    println!("Tracking presented frames for PID {pid}. Ctrl+C stops tracking.");
    while target_start(pid) == Some(started) {
        match capture(presentmon, pid) {
            Ok(samples) => {
                let selected = samples
                    .into_iter()
                    .max_by_key(|sample| sample.intervals)
                    .ok_or("no swap chain selected")?;
                if target_start(pid) != Some(started) {
                    break;
                }
                let mut temp = tempfile::NamedTempFile::new_in(folder)?;
                temp.write_all(&serde_json::to_vec(&selected)?)?;
                temp.persist(&file)?;
            }
            Err(error) => {
                // Publish no FPS for a process that is temporarily not presenting frames.
                let _ = fs::remove_file(&file);
                eprintln!("{error}");
                thread::sleep(Duration::from_secs(2));
            }
        }
    }
    let _ = fs::remove_file(file);
    Ok(())
}

pub fn latest() -> Result<Vec<Device>, Issue> {
    let missing = || Issue::new(Status::Unsupported, "FPS tracking is not active");
    let is_current = |sample: &FrameSample| {
        unix_ms()
            .checked_sub(sample.captured_at_unix_ms)
            .is_some_and(|age| age <= 10000)
    };
    let read_sample = |path: PathBuf| {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<FrameSample>(&bytes).ok())
            .filter(is_current)
    };
    let file = feed_path().map_err(|_| missing())?;
    let automatic = FOREGROUND_SAMPLE.lock().expect("FPS lock poisoned");
    let external;
    let sample = if let Some(sample) = automatic
        .as_ref()
        .filter(|sample| is_current(sample) && foreground() == Some(sample.pid))
    {
        sample
    } else {
        external = read_sample(file.clone())
            .or_else(|| {
                read_sample(file.with_file_name("fps-auto.json"))
                    .filter(|sample| foreground() == Some(sample.pid))
            })
            .ok_or_else(missing)?;
        &external
    };
    let mut device = Device::new(format!("frames:{}", sample.pid), &sample.app, "frames");
    device.add("fps", "Presented FPS", "FPS", Ok(sample.fps));
    device.add("frame_time", "Mean frame time", "ms", Ok(sample.frame_ms));
    device.add(
        "fps_1_percent_low",
        "1% low FPS",
        "FPS",
        sample
            .low_fps
            .ok_or_else(|| Issue::unavailable("fewer than 100 intervals")),
    );
    Ok(vec![device])
}

fn save_json(path: &Path, data: &impl Serialize) -> Result<(), Error> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or("invalid state path")?)?;
    temp.write_all(&serde_json::to_vec(data)?)?;
    temp.persist(path)?;
    Ok(())
}

pub(crate) fn foreground() -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };
    // SAFETY: We only read the current foreground window and its process ID.
    unsafe {
        let window = GetForegroundWindow();
        let mut pid = 0;
        if window.is_null() || GetWindowThreadProcessId(window, &mut pid) == 0 {
            return None;
        }
        (pid != 0).then_some(pid)
    }
}

pub(crate) fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase()
}

fn inside_game(exe: &Path, directories: &[PathBuf]) -> bool {
    let exe = path_key(exe);
    directories.iter().any(|directory| {
        let dir = path_key(directory);
        exe.strip_prefix(dir.trim_end_matches('\\'))
            .is_some_and(|suffix| suffix.starts_with('\\'))
    })
}

pub fn automatic(presentmon: &Path) -> Result<(), Error> {
    let presentmon = presentmon.canonicalize()?;
    if !presentmon.is_file()
        || !presentmon
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("select a local PresentMon executable".into());
    }
    let auto_feed = feed_path()?.with_file_name("fps-auto.json");
    fs::create_dir_all(auto_feed.parent().ok_or("invalid FPS feed folder")?)?;
    println!(
        "Tracking foreground Steam games and bound Windows game executables. Ctrl+C stops tracking."
    );

    track_foreground(
        &presentmon,
        &AtomicBool::new(false),
        |sample| match sample {
            Some(sample) => save_json(&auto_feed, &sample),
            None => {
                let _ = fs::remove_file(&auto_feed);
                Ok(())
            }
        },
    );
    Ok(())
}

pub(crate) fn integrated(stop: &AtomicBool) {
    if let Err(error) = run_integrated(stop) {
        crate::collector_log::record(&format!("automatic FPS: {error}"));
    }
}

fn run_integrated(stop: &AtomicBool) -> Result<(), Error> {
    use sha2::{Digest, Sha256};
    let path = env::current_exe()?.with_file_name("PresentMon.exe");
    // Allow execution but deny writes and replacement for the worker's lifetime.
    let mut binary = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    io::copy(&mut binary, &mut digest)?;
    if hex::encode(digest.finalize()) != PRESENTMON_SHA256 {
        return Err("PresentMon.exe does not match the supported release".into());
    }
    track_foreground(&path, stop, |sample| {
        *FOREGROUND_SAMPLE.lock().expect("FPS lock poisoned") = sample;
        Ok(())
    });
    Ok(())
}

fn track_foreground(
    presentmon: &Path,
    stop: &AtomicBool,
    mut publish: impl FnMut(Option<FrameSample>) -> Result<(), Error>,
) {
    let mut directories = Vec::new();
    let mut shortcuts = Vec::new();
    let mut last_scan = None;
    let mut previous_error = String::new();
    let mut last_target: Option<((u32, u64), bool)> = None;
    while !stop.load(Ordering::Acquire) {
        if last_scan
            .is_none_or(|time: std::time::Instant| time.elapsed() >= Duration::from_secs(30))
        {
            directories = crate::shortcuts::steam_game_directories();
            shortcuts = crate::shortcuts::windows_game_executables();
            last_scan = Some(std::time::Instant::now());
            last_target = None;
        }
        let selected = foreground().and_then(|id| {
            let identity = (id, target_start(id)?);
            if let Some((cached, recognized)) = last_target
                && cached == identity
            {
                return recognized.then_some(identity);
            }
            let pid = sysinfo::Pid::from_u32(id);
            let mut system = sysinfo::System::new().ok()?;
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
            );
            let process = system.process(pid)?;
            let exe = process.exe()?;
            let recognized = inside_game(exe, &directories)
                || shortcuts.iter().any(|path| path_key(path) == path_key(exe));
            last_target = Some((identity, recognized));
            recognized.then_some(identity)
        });
        let Some((pid, start)) = selected else {
            let _ = publish(None);
            thread::park_timeout(Duration::from_secs(1));
            continue;
        };

        let result = capture(presentmon, pid).and_then(|samples| {
            if foreground() != Some(pid) || target_start(pid) != Some(start) {
                return publish(None);
            }
            let selected = samples
                .into_iter()
                .max_by_key(|sample| sample.intervals)
                .ok_or("no tracked swap chain")?;
            publish(Some(selected))
        });
        match result {
            Ok(()) => previous_error.clear(),
            Err(error) => {
                let _ = publish(None);
                let detail = error.to_string();
                if detail != previous_error {
                    crate::collector_log::record(&format!("automatic FPS: {detail}"));
                    previous_error = detail;
                }
                thread::park_timeout(Duration::from_secs(2));
            }
        }
    }
    let _ = publish(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_windows_paths_without_matching_sibling_directories() {
        let roots = vec![PathBuf::from(r"D:\SteamLibrary\steamapps\common\Game")];
        assert!(inside_game(
            Path::new(r"d:\steamlibrary\steamapps\common\game\bin\game.exe"),
            &roots
        ));
        assert!(!inside_game(
            Path::new(r"D:\SteamLibrary\steamapps\common\GameBackup\browser.exe"),
            &roots
        ));
    }
}
