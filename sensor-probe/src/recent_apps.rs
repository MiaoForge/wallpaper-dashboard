use std::{
    fs,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{frames, shortcuts};
use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    Storage::Packaging::Appx::GetApplicationUserModelId,
    System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

fn packaged_id(pid: u32) -> Option<String> {
    let mut name = [0u16; 256];
    let mut length = name.len() as u32;
    // SAFETY: the process handle is closed here and name owns the writable buffer.
    let status = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let result = GetApplicationUserModelId(process, &mut length, name.as_mut_ptr());
        CloseHandle(process);
        result
    };
    if status != 0 || length < 2 || length as usize > name.len() {
        return None;
    }
    let name = String::from_utf16(&name[..length as usize - 1]).ok()?;
    name.contains('!')
        .then(|| format!("appx:{}", name.to_ascii_lowercase()))
}

#[derive(Deserialize, Serialize)]
struct Launch {
    id: String,
    started_at: u64,
}

static RECENT: OnceLock<Mutex<Vec<Launch>>> = OnceLock::new();

fn history() -> &'static Mutex<Vec<Launch>> {
    RECENT.get_or_init(|| {
        let saved = shortcuts::state_file("recent-apps.json")
            .ok()
            .and_then(|file| fs::read(file).ok());
        Mutex::new(
            saved
                .and_then(|bytes| {
                    serde_json::from_slice(&bytes).ok().or_else(|| {
                        let old: Vec<String> = serde_json::from_slice(&bytes).ok()?;
                        Some(
                            old.into_iter()
                                .map(|id| Launch { id, started_at: 0 })
                                .collect(),
                        )
                    })
                })
                .unwrap_or_default(),
        )
    })
}

pub(crate) fn ids() -> Vec<String> {
    history()
        .lock()
        .expect("recent apps lock poisoned")
        .iter()
        .map(|launch| launch.id.clone())
        .collect()
}

fn record(id: &str, started_at: u64) {
    let mut list = history().lock().expect("recent apps lock poisoned");
    if list
        .iter()
        .any(|item| item.id == id && item.started_at >= started_at)
    {
        return;
    }
    list.retain(|entry| entry.id != id);
    let position = list.partition_point(|entry| entry.started_at > started_at);
    list.insert(
        position,
        Launch {
            id: id.into(),
            started_at,
        },
    );
    list.truncate(32);
    if let Ok(file) = shortcuts::state_file("recent-apps.json")
        && let Ok(body) = serde_json::to_vec(&*list)
    {
        let _ = shortcuts::write_atomic(&file, &body);
    }
}

pub(crate) fn watch(stop: &AtomicBool) {
    let started = crate::telemetry::unix_ms() / 1000;
    let Ok(mut system) = sysinfo::System::new() else {
        return;
    };
    let mut targets = shortcuts::application_targets();
    crate::collector_log::record(&format!(
        "recent apps: loaded {} executable targets",
        targets.len()
    ));
    let mut steam_paths: Vec<_> = shortcuts::steam_game_directories()
        .iter()
        .map(|path| frames::path_key(path) + "\\")
        .collect();
    let mut last_scan = Instant::now();
    let mut previous = None;
    while !stop.load(Ordering::Acquire) {
        if last_scan.elapsed() >= Duration::from_secs(120) {
            targets = shortcuts::application_targets();
            steam_paths = shortcuts::steam_game_directories()
                .iter()
                .map(|path| frames::path_key(path) + "\\")
                .collect();
            last_scan = Instant::now();
            previous = None;
        }
        let foreground = frames::foreground();
        if foreground != previous
            && let Some(pid) = foreground.map(sysinfo::Pid::from_u32)
        {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            if let Some(process) = system.process(pid) {
                let launched = process.start_time();
                if launched >= started {
                    if let Some(id) = packaged_id(pid.as_u32()) {
                        record(&id, launched);
                    } else if let Some(path) = process.exe() {
                        let key = frames::path_key(path);
                        let file = key.rsplit('\\').next().unwrap_or_default();
                        if !matches!(
                            file,
                            "explorer.exe"
                                | "wallpaper-dashboard.exe"
                                | "wallpaper64.exe"
                                | "wallpaper32.exe"
                        ) && !steam_paths.iter().any(|path| key.starts_with(path))
                            && let Some(id) = targets.get(&key).or_else(|| {
                                let resolved = path.canonicalize().ok()?;
                                targets.get(&frames::path_key(&resolved))
                            })
                        {
                            record(id, launched);
                        }
                    }
                }
                previous = foreground;
            }
        } else if foreground.is_none() {
            previous = None;
        }
        thread::park_timeout(Duration::from_millis(500));
    }
}
