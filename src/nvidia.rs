use std::{env, path::PathBuf};

use nvml_wrapper::{
    Nvml,
    enum_wrappers::device::{Clock, TemperatureSensor},
    enums::device::SampleValue,
    error::NvmlError,
    structs::device::FieldId,
};

use crate::telemetry::{Device, Issue, Status};

// NVML_FI_DEV_MEMORY_TEMP from nvml.h.
const MEMORY_TEMP: FieldId = FieldId(82);

fn issue(error: NvmlError) -> Issue {
    let status = match error {
        NvmlError::NotSupported | NvmlError::FailedToLoadSymbol(_) => Status::Unsupported,
        NvmlError::NoPermission => Status::PermissionDenied,
        _ => Status::Unavailable,
    };
    Issue::new(status, error.to_string())
}

fn load() -> Result<Nvml, Issue> {
    let system =
        env::var_os("SystemRoot").map(|root| PathBuf::from(root).join(r"System32\nvml.dll"));
    let legacy = env::var_os("ProgramW6432")
        .or_else(|| env::var_os("ProgramFiles"))
        .map(|root| PathBuf::from(root).join(r"NVIDIA Corporation\NVSMI\nvml.dll"));
    let file = system
        .into_iter()
        .chain(legacy)
        .find(|path| path.is_file())
        .ok_or_else(|| Issue::new(Status::Unsupported, "NVIDIA NVML library was not found"))?;
    Nvml::builder()
        .lib_path(file.as_os_str())
        .init()
        .map_err(issue)
}

fn memory_temperature(gpu: &nvml_wrapper::Device<'_>) -> Result<f64, Issue> {
    let sample = gpu
        .field_values_for(&[MEMORY_TEMP])
        .map_err(issue)?
        .into_iter()
        .next()
        .ok_or_else(|| Issue::unavailable("NVML returned no memory temperature field"))?
        .map_err(issue)?;
    let value = match sample.value.map_err(issue)? {
        SampleValue::F64(value) => value,
        SampleValue::U32(value) => f64::from(value),
        SampleValue::U64(value) => value as f64,
        SampleValue::I64(value) => value as f64,
    };
    Ok(value)
}

pub fn sample() -> Result<Vec<Device>, Issue> {
    let nvml = load()?;
    let count = nvml.device_count().map_err(issue)?;
    if count == 0 {
        return Err(Issue::new(Status::Unsupported, "no NVIDIA adapters found"));
    }
    let mut devices = Vec::new();
    for index in 0..count {
        let fallback = format!("gpu:nvidia:index:{index}");
        let gpu = match nvml.device_by_index(index) {
            Ok(gpu) => gpu,
            Err(error) => {
                let mut device = Device::new(fallback, format!("NVIDIA GPU {index}"), "gpu");
                device.add("usage", "GPU utilization", "%", Err(issue(error)));
                devices.push(device);
                continue;
            }
        };
        let id = gpu
            .uuid()
            .map(|uuid| format!("gpu:nvidia:{uuid}"))
            .unwrap_or(fallback);
        let mut device = Device::new(
            id,
            gpu.name().unwrap_or_else(|_| format!("NVIDIA GPU {index}")),
            "gpu",
        );
        device.add(
            "usage",
            "GPU utilization",
            "%",
            gpu.utilization_rates()
                .map(|value| f64::from(value.gpu))
                .map_err(issue),
        );
        device.add(
            "core_temperature",
            "GPU core temperature",
            "°C",
            gpu.temperature(TemperatureSensor::Gpu)
                .map(f64::from)
                .map_err(issue),
        );
        device.add(
            "memory_temperature",
            "VRAM temperature",
            "°C",
            memory_temperature(&gpu),
        );
        device.add(
            "board_power",
            "GPU board power",
            "W",
            gpu.power_usage()
                .map(|mw| f64::from(mw) / 1000.0)
                .map_err(issue),
        );
        for (id, label, clock) in [
            ("core_clock", "GPU clock", Clock::Graphics),
            ("memory_clock", "VRAM clock", Clock::Memory),
        ] {
            device.add(
                id,
                label,
                "MHz",
                gpu.clock_info(clock).map(f64::from).map_err(issue),
            );
        }
        match gpu.memory_info() {
            Ok(memory) => {
                device.add("memory_used", "VRAM used", "B", Ok(memory.used as f64));
                device.add("memory_total", "VRAM total", "B", Ok(memory.total as f64));
            }
            Err(error) => device.add("memory_used", "VRAM used", "B", Err(issue(error))),
        }
        match gpu.num_fans() {
            Ok(count) => {
                for fan in 0..count {
                    device.add(
                        format!("fan:{fan}:target_speed"),
                        format!("GPU fan {fan} target speed"),
                        "RPM",
                        gpu.fan_speed_rpm(fan).map(f64::from).map_err(issue),
                    );
                    device.add(
                        format!("fan:{fan}:target_percent"),
                        format!("GPU fan {fan} target speed"),
                        "%",
                        gpu.fan_speed(fan).map(f64::from).map_err(issue),
                    );
                }
            }
            Err(error) => device.add(
                "fan:0:target_speed",
                "GPU fan target speed",
                "RPM",
                Err(issue(error)),
            ),
        }
        devices.push(device);
    }
    Ok(devices)
}
