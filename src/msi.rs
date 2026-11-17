use std::{
    collections::HashSet,
    ffi::c_void,
    io,
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use crate::telemetry::{Device, Issue, Status};
use serde::Deserialize;

const MAP_NAME: &str = r"Global\SharedMemory_CC_Engine";
const MAP_BYTES: usize = 256 * 1024;
const FILE_MAP_READ: u32 = 4;
static SPINNING_CHANNELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    fn MapViewOfFile(
        handle: *mut c_void,
        access: u32,
        high: u32,
        low: u32,
        size: usize,
    ) -> *const c_void;
    fn UnmapViewOfFile(address: *const c_void) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

struct View(*const c_void);

impl View {
    fn open() -> io::Result<Self> {
        let name: Vec<_> = MAP_NAME.encode_utf16().chain([0]).collect();
        // SAFETY: The name is NUL-terminated. The mapping and view are both read-only.
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, MAP_BYTES);
            let result = if view.is_null() {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(view))
            };
            CloseHandle(handle);
            result
        }
    }

    fn snapshot(&self) -> Vec<u16> {
        // SAFETY: The mapped range has MAP_BYTES readable bytes and is aligned.
        // Volatile reads observe the external writer without creating a shared slice.
        let source = self.0.cast::<u16>();
        let mut snapshot = Vec::with_capacity(MAP_BYTES / 2);
        for index in 0..MAP_BYTES / 2 {
            let unit = unsafe { source.add(index).read_volatile() };
            if unit == 0 {
                break;
            }
            snapshot.push(unit);
        }
        snapshot
    }
}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: The address came from MapViewOfFile and is still mapped.
        unsafe { UnmapViewOfFile(self.0) };
    }
}

#[derive(Deserialize)]
struct Fans {
    #[serde(rename = "FanCounts")]
    count: usize,
    #[serde(rename = "CurFanID", default)]
    ids: Vec<u32>,
    #[serde(rename = "FanName")]
    names: Vec<FanName>,
    #[serde(rename = "CurFanSpeed")]
    speeds: Vec<f64>,
}

#[derive(Deserialize)]
struct FanName {
    #[serde(rename = "byFan_Name")]
    bytes: Vec<u8>,
    #[serde(rename = "FanNameLenth")]
    len: usize,
}

pub fn sample() -> Result<Vec<Device>, Issue> {
    let view = View::open().map_err(|error| {
        if error.kind() == io::ErrorKind::PermissionDenied {
            Issue::new(
                Status::PermissionDenied,
                format!("MSI Center shared memory requires an administrator terminal ({error})"),
            )
        } else {
            Issue::unavailable(format!("could not open MSI Center shared memory ({error})"))
        }
    })?;

    // The MSI background service can update the document during a snapshot.
    // Retry a malformed snapshot instead of treating it as missing sensors.
    for _ in 0..5 {
        let snapshot = view.snapshot();
        if let Ok(json) = String::from_utf16(&snapshot)
            && let Ok(fans) = serde_json::from_str::<Fans>(&json)
            && fans.count > 0
            && fans.count <= fans.names.len().min(fans.speeds.len())
        {
            let mut spinning = SPINNING_CHANNELS
                .get_or_init(Mutex::default)
                .lock()
                .expect("fan channel lock poisoned");
            let mut device = Device::new(
                "motherboard:system",
                "MSI Center motherboard",
                "motherboard",
            );
            for (index, (name, rpm)) in fans
                .names
                .iter()
                .zip(&fans.speeds)
                .take(fans.count)
                .enumerate()
            {
                let label = name
                    .bytes
                    .get(..name.len)
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .filter(|label| {
                        !label.trim().is_empty() && !label.chars().any(char::is_control)
                    })
                    .map_or_else(|| format!("Channel {}", index + 1), str::to_owned);
                let id = fans
                    .ids
                    .get(index)
                    .filter(|&&id| {
                        id != 0
                            && fans
                                .ids
                                .iter()
                                .filter(|&candidate| candidate == &id)
                                .count()
                                == 1
                    })
                    .map_or_else(
                        || format!("fan:index:{index}:speed"),
                        |id| format!("fan:msi:{id}:speed"),
                    );
                let reading = if rpm.is_finite() && *rpm >= 0.0 {
                    Ok(*rpm)
                } else {
                    Err(Issue::unavailable("no valid tachometer reading"))
                };
                if reading.as_ref().is_ok_and(|speed| *speed > 0.0) {
                    spinning.insert(id.clone());
                }
                let seen_spinning = spinning.contains(&id);
                device.add_fan(id, label, reading, seen_spinning);
            }
            return Ok(vec![device]);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(Issue::unavailable(
        "MSI Center did not expose a valid fan snapshot",
    ))
}
