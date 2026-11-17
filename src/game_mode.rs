use std::{
    io,
    mem::size_of,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    },
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
};

static MANUAL: AtomicBool = AtomicBool::new(false);
static LEAGUE_RUNNING: AtomicBool = AtomicBool::new(false);
static GPU_WORKERS: AtomicU8 = AtomicU8::new(0);
static LAUNCH_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

pub fn set_manual(enabled: bool) {
    MANUAL.store(enabled, Ordering::SeqCst);
}

pub(crate) fn active() -> bool {
    MANUAL.load(Ordering::SeqCst)
        || LEAGUE_RUNNING.load(Ordering::SeqCst)
        || LAUNCH_UNTIL
            .lock()
            .unwrap()
            .is_some_and(|until| Instant::now() < until)
}

pub(crate) fn worker_bit(source: &str) -> u8 {
    match source {
        "amd_adlx" => 1,
        "nvidia_nvml" => 2,
        "windows_gpu" => 4,
        _ => 0,
    }
}

pub(crate) fn enter_gpu_worker(bit: u8) {
    GPU_WORKERS.fetch_or(bit, Ordering::SeqCst);
}

pub(crate) fn leave_gpu_worker(bit: u8) {
    GPU_WORKERS.fetch_and(!bit, Ordering::SeqCst);
}

pub(crate) fn launch_league(launch: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    *LAUNCH_UNTIL.lock().unwrap() = Some(Instant::now() + Duration::from_secs(60));
    let deadline = Instant::now() + Duration::from_secs(5);
    while GPU_WORKERS.load(Ordering::SeqCst) != 0 {
        if Instant::now() >= deadline {
            *LAUNCH_UNTIL.lock().unwrap() = None;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "等待 GPU 采集停止超时",
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
    let result = launch();
    if result.is_err() {
        *LAUNCH_UNTIL.lock().unwrap() = None;
    }
    result
}

pub(crate) fn refresh() {
    if let Some(running) = league_running() {
        let previous = LEAGUE_RUNNING.load(Ordering::SeqCst);
        if previous != running {
            let mut until = LAUNCH_UNTIL.lock().unwrap();
            *until = (!running).then(|| Instant::now() + Duration::from_secs(3));
            LEAGUE_RUNNING.store(running, Ordering::SeqCst);
            crate::collector_log::record(if running {
                "game compatibility: League client detected"
            } else {
                "game compatibility: League client exited"
            });
        }
    }
}

pub(crate) fn watch(stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        refresh();
        thread::park_timeout(Duration::from_millis(500));
    }
}

fn league_running() -> Option<bool> {
    // Read only the executable names from the Windows process list.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let first = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    let mut live = first;
    let mut found = false;
    while live {
        let end = entry
            .szExeFile
            .iter()
            .position(|&ch| ch == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        let client = [
            "LeagueClient.exe",
            "LeagueClientUx.exe",
            "League of Legends.exe",
        ]
        .iter()
        .any(|target| name.eq_ignore_ascii_case(target));
        if client
            || (name.eq_ignore_ascii_case("launcher.exe")
                && is_wegame_league_launcher(entry.th32ProcessID))
        {
            found = true;
            break;
        }
        live = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    first.then_some(found)
}

fn is_wegame_league_launcher(pid: u32) -> bool {
    let mut path = [0u16; 1024];
    let mut size = path.len() as u32;
    // Only ask Windows for the executable path, then close the handle.
    let success = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let result = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size);
        CloseHandle(process);
        result != 0
    };
    success
        && String::from_utf16_lossy(&path[..size as usize])
            .to_ascii_lowercase()
            .ends_with(r"\lol\wegamelauncher\launcher.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_gpu_worker_and_clears_pause_on_launch_failure() {
        let bit = worker_bit("windows_gpu");
        enter_gpu_worker(bit);
        thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(2);
                while !active() && Instant::now() < deadline {
                    thread::yield_now();
                }
                leave_gpu_worker(bit);
            });
            let result = launch_league(|| {
                assert!(active());
                assert_eq!(GPU_WORKERS.load(Ordering::SeqCst), 0);
                Err(io::Error::other("test launch failure"))
            });
            assert_eq!(result.unwrap_err().to_string(), "test launch failure");
            assert!(!active());
        });
    }
}
