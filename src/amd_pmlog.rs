use std::{cell::RefCell, ffi::c_void, mem::size_of, ptr};

use libloading::os::windows::{LOAD_LIBRARY_SEARCH_SYSTEM32, Library};

use crate::telemetry::{Issue, Status};

const ADL_MAX_PATH: usize = 256;
const ADL_PMLOG_MAX_SENSORS: usize = 256;
const ADL_PMLOG_TEMPERATURE_MEM: usize = 9;
const ADL_PMLOG_INFO_ACTIVITY_GFX: usize = 19;
const ADL_PMLOG_BOARD_POWER: usize = 73;

type Context = *mut c_void;
type Allocate = unsafe extern "system" fn(i32) -> *mut c_void;
type Create = unsafe extern "system" fn(Allocate, i32, *mut Context) -> i32;
type Destroy = unsafe extern "system" fn(Context) -> i32;
type Count = unsafe extern "system" fn(Context, *mut i32) -> i32;
type Info = unsafe extern "system" fn(Context, *mut AdapterInfo, i32) -> i32;
type Query = unsafe extern "system" fn(Context, i32, *mut PMLogData) -> i32;

#[repr(C)]
struct AdapterInfo {
    size: i32,
    index: i32,
    udid: [u8; ADL_MAX_PATH],
    bus: i32,
    device: i32,
    function: i32,
    vendor: i32,
    name: [u8; ADL_MAX_PATH],
    display_name: [u8; ADL_MAX_PATH],
    present: i32,
    exists: i32,
    driver_path: [u8; ADL_MAX_PATH],
    driver_path_ext: [u8; ADL_MAX_PATH],
    pnp_string: [u8; ADL_MAX_PATH],
    os_display_index: i32,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Sensor {
    supported: i32,
    value: i32,
}

#[repr(C)]
pub(crate) struct PMLogData {
    size: i32,
    sensors: [Sensor; ADL_PMLOG_MAX_SENSORS],
}

impl PMLogData {
    pub(crate) fn graphics_activity(&self) -> Result<f64, Issue> {
        self.reading(ADL_PMLOG_INFO_ACTIVITY_GFX, "graphics activity", 0..=100)
    }

    pub(crate) fn memory_temperature(&self) -> Result<f64, Issue> {
        self.reading(ADL_PMLOG_TEMPERATURE_MEM, "memory temperature", 1..=150)
    }

    pub(crate) fn board_power(&self) -> Result<f64, Issue> {
        self.reading(ADL_PMLOG_BOARD_POWER, "board power", 0..=2000)
    }

    fn reading(
        &self,
        id: usize,
        label: &str,
        valid: std::ops::RangeInclusive<i32>,
    ) -> Result<f64, Issue> {
        let sensor = self.sensors[id];
        if sensor.supported != 1 {
            return Err(Issue::new(
                Status::Unsupported,
                format!("ADL PMLog {label} unavailable"),
            ));
        }
        if !valid.contains(&sensor.value) {
            return Err(Issue::unavailable(format!("ADL returned invalid {label}")));
        }
        Ok(f64::from(sensor.value))
    }
}

struct PmLog {
    context: Context,
    destroy: Destroy,
    query: Query,
    adapters: Vec<AdapterInfo>,
    _library: Library,
}

thread_local! {
    static PMLOG: RefCell<Option<Result<PmLog, Issue>>> = const { RefCell::new(None) };
}

pub(crate) fn release() {
    PMLOG.with(|state| {
        state.borrow_mut().take();
    });
}

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

unsafe extern "system" fn allocate(size: i32) -> *mut c_void {
    if size <= 0 {
        return ptr::null_mut();
    }
    // ADL's callback uses the CRT allocator, as in its SDK examples.
    unsafe { malloc(size as usize) }
}

impl Drop for PmLog {
    fn drop(&mut self) {
        // The ADL library is still loaded and owns this context.
        unsafe { (self.destroy)(self.context) };
    }
}

fn check(operation: &str, status: i32) -> Result<(), Issue> {
    if status == 0 {
        Ok(())
    } else {
        Err(Issue::unavailable(format!(
            "{operation}: ADL status {status}"
        )))
    }
}

impl PmLog {
    fn open() -> Result<Self, Issue> {
        let load_error = |error: libloading::Error| Issue::unavailable(error.to_string());
        // Use the display driver's system library and the public ADL SDK ABI.
        let library =
            unsafe { Library::load_with_flags("atiadlxx.dll", LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(load_error)?;
        let (create, destroy, count, info, query) = unsafe {
            (
                *library
                    .get::<Create>(b"ADL2_Main_Control_Create\0")
                    .map_err(load_error)?,
                *library
                    .get::<Destroy>(b"ADL2_Main_Control_Destroy\0")
                    .map_err(load_error)?,
                *library
                    .get::<Count>(b"ADL2_Adapter_NumberOfAdapters_Get\0")
                    .map_err(load_error)?,
                *library
                    .get::<Info>(b"ADL2_Adapter_AdapterInfo_Get\0")
                    .map_err(load_error)?,
                *library
                    .get::<Query>(b"ADL2_New_QueryPMLogData_Get\0")
                    .map_err(load_error)?,
            )
        };
        let mut context = ptr::null_mut();
        unsafe {
            check(
                "ADL2_Main_Control_Create",
                create(allocate, 1, &mut context),
            )?
        };
        if context.is_null() {
            return Err(Issue::unavailable("ADL did not return a context"));
        }
        let mut result = Self {
            context,
            destroy,
            query,
            adapters: Vec::new(),
            _library: library,
        };
        let mut adapter_count = 0;
        unsafe {
            check(
                "ADL2_Adapter_NumberOfAdapters_Get",
                count(context, &mut adapter_count),
            )?
        };
        if !(1..=256).contains(&adapter_count) {
            return Err(Issue::unavailable("unexpected ADL adapter count"));
        }
        let row_size = size_of::<AdapterInfo>() as i32;
        result.adapters.resize_with(adapter_count as usize, || {
            // AdapterInfo consists entirely of integer fields and character buffers.
            let mut adapter: AdapterInfo = unsafe { std::mem::zeroed() };
            adapter.size = row_size;
            adapter
        });
        unsafe {
            check(
                "ADL2_Adapter_AdapterInfo_Get",
                info(
                    context,
                    result.adapters.as_mut_ptr(),
                    adapter_count * row_size,
                ),
            )?;
        }
        Ok(result)
    }

    fn snapshot(&self, name: &str) -> Result<PMLogData, Issue> {
        let mut matches = self.adapters.iter().filter(|adapter| {
            let end = adapter
                .name
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(ADL_MAX_PATH);
            adapter.exists == 1
                && adapter.bus >= 0
                && adapter.name[..end].eq_ignore_ascii_case(name.as_bytes())
        });
        let adapter = matches
            .next()
            .ok_or_else(|| Issue::unavailable("no matching ADL adapter"))?;
        let address = (adapter.bus, adapter.device, adapter.function);
        if matches.any(|other| (other.bus, other.device, other.function) != address) {
            return Err(Issue::unavailable(
                "multiple physical ADL adapters share this name",
            ));
        }
        let mut output = PMLogData {
            size: size_of::<PMLogData>() as i32,
            sensors: [Sensor {
                supported: 0,
                value: 0,
            }; ADL_PMLOG_MAX_SENSORS],
        };
        // The driver fills the documented PMLog snapshot for the selected adapter.
        unsafe {
            check(
                "ADL2_New_QueryPMLogData_Get",
                (self.query)(self.context, adapter.index, &mut output),
            )?
        };
        Ok(output)
    }
}

pub(crate) fn snapshot(name: &str) -> Result<PMLogData, Issue> {
    PMLOG.with(|state| {
        let mut state = state.borrow_mut();
        match state.get_or_insert_with(PmLog::open) {
            Ok(pmlog) => pmlog.snapshot(name),
            Err(issue) => Err(issue.clone()),
        }
    })
}
