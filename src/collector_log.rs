use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

use crate::telemetry::unix_ms;

const MAX_BYTES: u64 = 256 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

pub fn path() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA").map(|root| {
        PathBuf::from(root)
            .join("WallpaperDashboard")
            .join("collector.log")
    })
}

pub fn record(message: &str) {
    let Some(path) = path() else { return };
    let Ok(_locked) = LOCK.lock() else { return };
    let Some(folder) = path.parent() else { return };
    if fs::create_dir_all(folder).is_err() {
        return;
    }

    if fs::metadata(&path).is_ok_and(|file| file.len() >= MAX_BYTES) {
        let previous = path.with_extension("previous.log");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&path, previous);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let message = message.replace(['\r', '\n'], " ");
        let _ = writeln!(file, "{} {message}", unix_ms());
    }
}
