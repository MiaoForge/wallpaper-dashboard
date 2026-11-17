use std::{
    collections::VecDeque,
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

fn promote(list: &mut Vec<Launch>, id: &str, started_at: u64) -> bool {
    if list.first().is_some_and(|item| item.id == id) {
        return false;
    }
    list.retain(|entry| entry.id != id);
    list.insert(
        0,
        Launch {
            id: id.into(),
            started_at,
        },
    );
    list.truncate(32);
    true
}

fn persist() -> std::io::Result<()> {
    let file = shortcuts::state_file("recent-apps.json")?;
    let body = serde_json::to_vec(&*history().lock().expect("recent apps lock poisoned"))?;
    shortcuts::write_atomic(&file, &body)
}

pub(crate) fn watch(stop: &AtomicBool) {
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
    let mut resolved = VecDeque::new();
    let mut pending_save: Option<Instant> = None;
    while !stop.load(Ordering::Acquire) {
        if last_scan.elapsed() >= Duration::from_secs(120) {
            targets = shortcuts::application_targets();
            steam_paths = shortcuts::steam_game_directories()
                .iter()
                .map(|path| frames::path_key(path) + "\\")
                .collect();
            last_scan = Instant::now();
            resolved.clear();
        }
        let foreground = frames::foreground();
        if foreground != previous
            && let Some(pid) = foreground.map(sysinfo::Pid::from_u32)
        {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
            );
            if let Some(process) = system.process(pid) {
                let process_key = (pid, process.start_time());
                let cached = resolved.iter().position(|(key, _)| *key == process_key);
                let id = if let Some(index) = cached {
                    resolved.remove(index).map(|(_, id)| id)
                } else {
                    packaged_id(pid.as_u32()).or_else(|| {
                        let path = process.exe()?;
                        let key = frames::path_key(path);
                        let file = key.rsplit('\\').next().unwrap_or_default();
                        if !matches!(
                            file,
                            "explorer.exe"
                                | "wallpaper-dashboard.exe"
                                | "wallpaper64.exe"
                                | "wallpaper32.exe"
                        ) && !steam_paths.iter().any(|path| key.starts_with(path))
                        {
                            targets
                                .get(&key)
                                .or_else(|| {
                                    let resolved = path.canonicalize().ok()?;
                                    targets.get(&frames::path_key(&resolved))
                                })
                                .cloned()
                        } else {
                            None
                        }
                    })
                };
                if let Some(id) = id {
                    if promote(
                        &mut history().lock().expect("recent apps lock poisoned"),
                        &id,
                        crate::telemetry::unix_ms() / 1000,
                    ) {
                        crate::shortcut_http::dock_changed();
                        pending_save.get_or_insert_with(Instant::now);
                    }
                    resolved.push_back((process_key, id));
                    if resolved.len() > 128 {
                        resolved.pop_front();
                    }
                }
                previous = foreground;
            }
        } else if foreground.is_none() {
            previous = None;
        }
        if pending_save.is_some_and(|since| since.elapsed() >= Duration::from_secs(2)) {
            pending_save = if persist().is_ok() {
                None
            } else {
                Some(Instant::now())
            };
        }
        thread::park_timeout(Duration::from_millis(500));
    }
    if pending_save.is_some() {
        let _ = persist();
    }
}

#[cfg(test)]
mod tests {
    use super::promote;

    #[test]
    fn revisiting_an_existing_app_moves_it_to_the_front() {
        let mut list = Vec::new();
        assert!(promote(&mut list, "first", 1));
        assert!(promote(&mut list, "second", 2));
        assert!(promote(&mut list, "first", 2));
        assert!(!promote(&mut list, "first", 3));
        assert_eq!(
            list.iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}
