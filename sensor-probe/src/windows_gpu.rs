use std::{cell::RefCell, collections::HashMap, mem::size_of, ptr};

use windows::{
    Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1},
    core::PCWSTR,
};
use windows_sys::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW,
};

use crate::telemetry::{Device, Issue};
use windows_sys::{
    Wdk::Graphics::Direct3D::{
        D3DKMT_ADAPTER_PERFDATA, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID,
        D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid,
        D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
    },
    Win32::Foundation::LUID,
};

struct EngineQuery {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

thread_local! {
    static ENGINE_QUERY: RefCell<Option<EngineQuery>> = const { RefCell::new(None) };
}

pub(crate) fn release() {
    ENGINE_QUERY.with(|query| {
        query.borrow_mut().take();
    });
    crate::amd_pmlog::release();
}

impl Drop for EngineQuery {
    fn drop(&mut self) {
        // The counter belongs to this query and is released with it.
        unsafe { PdhCloseQuery(self.query) };
    }
}

fn check(operation: &str, status: u32) -> Result<(), Issue> {
    if status == 0 {
        Ok(())
    } else {
        Err(Issue::unavailable(format!("{operation}: 0x{status:08X}")))
    }
}

impl EngineQuery {
    fn open() -> Result<Self, Issue> {
        let mut query = ptr::null_mut();
        // PDH initializes these handles and copies the nul-terminated counter path.
        unsafe { check("PdhOpenQuery", PdhOpenQueryW(ptr::null(), 0, &mut query))? };
        let mut result = Self {
            query,
            counter: ptr::null_mut(),
        };
        let path = windows::core::w!(r"\GPU Engine(*)\Utilization Percentage");
        unsafe {
            check(
                "PdhAddEnglishCounter",
                PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut result.counter),
            )?;
            check("PdhCollectQueryData", PdhCollectQueryData(query))?;
        }
        Ok(result)
    }

    fn engines(&self) -> Result<HashMap<(u32, u32, u32, u32), f64>, Issue> {
        let mut bytes = 0;
        let mut count = 0;
        // The first call obtains the required size. An aligned buffer holds rows and names.
        unsafe { check("PdhCollectQueryData", PdhCollectQueryData(self.query))? };
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut bytes,
                &mut count,
                ptr::null_mut(),
            )
        };
        if status != PDH_MORE_DATA {
            check("PdhGetFormattedCounterArray", status)?;
            return Ok(HashMap::new());
        }
        let mut buffer = vec![
            PDH_FMT_COUNTERVALUE_ITEM_W::default();
            (bytes as usize).div_ceil(size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>())
        ];
        unsafe {
            check(
                "PdhGetFormattedCounterArray",
                PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut bytes,
                    &mut count,
                    buffer.as_mut_ptr(),
                ),
            )?;
        }
        let mut engines = HashMap::new();
        for item in buffer.iter().take(count as usize) {
            if item.szName.is_null() || item.FmtValue.CStatus > 1 {
                continue;
            }
            // PDH returns a terminated instance name and a double for PDH_FMT_DOUBLE.
            let (name, value) = unsafe {
                (
                    PCWSTR(item.szName).to_string(),
                    item.FmtValue.Anonymous.doubleValue,
                )
            };
            if let Some(key) = name.ok().as_deref().and_then(engine_key)
                && value.is_finite()
                && value >= 0.0
            {
                *engines.entry(key).or_insert(0.0) += value;
            }
        }
        Ok(engines)
    }
}

fn engine_key(name: &str) -> Option<(u32, u32, u32, u32)> {
    let (_, suffix) = name.split_once("_luid_")?;
    let mut parts = suffix.split('_');
    let high = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    let low = u32::from_str_radix(parts.next()?.strip_prefix("0x")?, 16).ok()?;
    if parts.next()? != "phys" {
        return None;
    }
    let physical = parts.next()?.parse().ok()?;
    if parts.next()? != "eng" {
        return None;
    }
    let engine = parts.next()?.parse().ok()?;
    Some((high, low, physical, engine))
}

pub(crate) fn sample_usage() -> Result<Vec<Device>, Issue> {
    let engines = ENGINE_QUERY.with(|query| {
        let mut query = query.borrow_mut();
        match query.as_ref() {
            Some(query) => query.engines(),
            None => {
                *query = Some(EngineQuery::open()?);
                Err(Issue::unavailable(
                    "GPU engine counters need a second sample",
                ))
            }
        }
    })?;
    sample_adapters(|device, luid| {
        let usage = engines
            .iter()
            .filter(|((h, l, _, _), _)| (*h, *l) == (luid.HighPart as u32, luid.LowPart))
            .map(|(_, value)| *value)
            .reduce(f64::max)
            .map(|value| value.min(100.0))
            .ok_or_else(|| Issue::unavailable("no GPU engine readings for adapter"));
        device.add("usage", "GPU utilization", "%", usage);
    })
}

pub(crate) fn sample_temperature() -> Result<Vec<Device>, Issue> {
    sample_adapters(|device, luid| {
        device.add(
            "main_temperature",
            "GPU main temperature",
            "°C",
            main_temperature(luid),
        );
    })
}

pub(crate) fn sample_temperature_with_memory() -> Result<Vec<Device>, Issue> {
    sample_temperature_with_pmlog(false)
}

pub(crate) fn sample_temperature_with_memory_and_power() -> Result<Vec<Device>, Issue> {
    sample_temperature_with_pmlog(true)
}

fn sample_temperature_with_pmlog(board_power: bool) -> Result<Vec<Device>, Issue> {
    let graphics_activity = std::env::args_os().any(|arg| arg == "--amd-pmlog-usage");
    let mut devices = sample_temperature()?;
    let mut readings = HashMap::new();
    for device in &mut devices {
        let snapshot = readings
            .entry(device.name.clone())
            .or_insert_with(|| crate::amd_pmlog::snapshot(&device.name));
        if graphics_activity {
            device.add(
                "pmlog_usage",
                "GPU graphics activity",
                "%",
                snapshot
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|data| data.graphics_activity()),
            );
        }
        device.add(
            "memory_temperature",
            "GPU memory temperature",
            "°C",
            snapshot
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|data| data.memory_temperature()),
        );
        if board_power {
            device.add(
                "board_power",
                "GPU total board power",
                "W",
                snapshot
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|data| data.board_power()),
            );
        }
    }
    Ok(devices)
}

fn main_temperature(luid: LUID) -> Result<f64, Issue> {
    let mut adapter = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    // Open a handle to the adapter and query its first physical GPU's performance data.
    let opened = unsafe { D3DKMTOpenAdapterFromLuid(&mut adapter) };
    if opened < 0 {
        return Err(Issue::unavailable(format!(
            "D3DKMTOpenAdapterFromLuid: 0x{:08X}",
            opened as u32
        )));
    }
    let mut data = D3DKMT_ADAPTER_PERFDATA::default();
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: adapter.hAdapter,
        Type: KMTQAITYPE_ADAPTERPERFDATA,
        pPrivateDriverData: ptr::from_mut(&mut data).cast(),
        PrivateDriverDataSize: size_of::<D3DKMT_ADAPTER_PERFDATA>() as u32,
    };
    let status = unsafe { D3DKMTQueryAdapterInfo(&mut query) };
    unsafe {
        D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
            hAdapter: adapter.hAdapter,
        })
    };
    if status < 0 {
        return Err(Issue::unavailable(format!(
            "D3DKMTQueryAdapterInfo: 0x{:08X}",
            status as u32
        )));
    }
    if !(1..=1500).contains(&data.Temperature) {
        return Err(Issue::unavailable(
            "driver did not provide a usable GPU temperature",
        ));
    }
    Ok(f64::from(data.Temperature) / 10.0)
}

fn sample_adapters(mut add_metrics: impl FnMut(&mut Device, LUID)) -> Result<Vec<Device>, Issue> {
    // DXGI enumerates local adapters and exposes the LUID used by Windows GPU telemetry.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|error| Issue::unavailable(error.to_string()))?;
    let mut devices = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.VendorId != 0x1002 {
            continue;
        }
        let high = desc.AdapterLuid.HighPart as u32;
        let low = desc.AdapterLuid.LowPart;
        let name = String::from_utf16_lossy(&desc.Description);
        let mut device = Device::new(
            format!("dxgi:{high:08x}:{low:08x}"),
            name.trim_end_matches('\0'),
            "gpu",
        );
        add_metrics(
            &mut device,
            LUID {
                HighPart: desc.AdapterLuid.HighPart,
                LowPart: low,
            },
        );
        device.add(
            "memory_total",
            "Dedicated video memory",
            "B",
            Ok(desc.DedicatedVideoMemory as f64),
        );
        devices.push(device);
    }
    Ok(devices)
}
