use std::{
    env,
    io::{self, Read},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::PathBuf,
    process::{Command, Output, Stdio},
    thread,
};

use adlx_lite::{
    AdlxHelper, ffi,
    interface::{Interface, InterfaceImpl},
};
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::WaitForSingleObject,
};

use crate::telemetry::{Device, Issue, Status};

#[link(name = "shell32")]
unsafe extern "system" {
    fn IsUserAnAdmin() -> i32;
}

// adlx-lite 1.0 exposes the SDK ABI for this extension but has no safe wrapper
// for GPUMemoryTemperature. QueryInterface acquires an owned reference.
#[repr(transparent)]
struct MemoryTemperature(InterfaceImpl);

// SAFETY: This owns an ADLX interface pointer with the matching SDK vtable.
unsafe impl Interface for MemoryTemperature {
    type Impl = ffi::IADLXGPUMetrics1;
    type Vtable = ffi::IADLXGPUMetrics1Vtbl;
    const IID: &'static str = "IADLXGPUMetrics1";
}

impl MemoryTemperature {
    fn read(&self) -> Result<f64, Issue> {
        let read = self.vtable().GPUMemoryTemperature.ok_or_else(|| {
            Issue::new(
                Status::Unsupported,
                "driver has no memory-temperature function",
            )
        })?;
        let mut value = f64::NAN;
        // SAFETY: The interface was obtained via QueryInterface and value is
        // writable storage of the SDK's adlx_double type.
        let status = unsafe { read(self.as_raw(), &mut value) };
        adlx_lite::Error::from_result(status).map_err(adlx_issue)?;
        Ok(value)
    }
}

fn adlx_issue(error: adlx_lite::Error) -> Issue {
    let detail = error.to_string();
    match detail.as_str() {
        "The asked feature is not supported"
        | "This result indicates that an unknown interface was asked" => {
            Issue::new(Status::Unsupported, detail)
        }
        _ => Issue::unavailable(detail),
    }
}

pub fn gpu() -> Result<Vec<Device>, Issue> {
    let driver = AdlxHelper::new().map_err(|error| Issue::unavailable(error.to_string()))?;
    let system = driver.system();
    let services = system
        .performance_monitoring_services()
        .map_err(adlx_issue)?;
    let gpus = system.gpus().map_err(adlx_issue)?;
    let mut devices = Vec::new();

    for (index, adapter) in gpus.iter().enumerate() {
        let id = adapter
            .unique_id()
            .map(|id| format!("gpu:amd:{id}"))
            .unwrap_or_else(|_| format!("gpu:amd:index:{index}"));
        let mut device = Device::new(
            id,
            adapter.name().unwrap_or("Unnamed Radeon adapter"),
            "gpu",
        );
        device.add(
            "memory_total",
            "VRAM total",
            "B",
            adapter
                .total_vram()
                .map(|mb| f64::from(mb) * 1_048_576.0)
                .map_err(adlx_issue),
        );
        match services.current_gpu_metrics(&adapter) {
            Ok(metrics) => {
                device.add(
                    "usage",
                    "GPU utilization",
                    "%",
                    metrics.usage().map_err(adlx_issue),
                );
                device.add(
                    "core_temperature",
                    "GPU core temperature",
                    "°C",
                    metrics.temperature().map_err(adlx_issue),
                );
                device.add(
                    "hotspot_temperature",
                    "GPU hotspot temperature",
                    "°C",
                    metrics.hotspot_temperature().map_err(adlx_issue),
                );
                device.add(
                    "memory_temperature",
                    "VRAM temperature",
                    "°C",
                    metrics
                        .cast::<MemoryTemperature>()
                        .map_err(adlx_issue)
                        .and_then(|sensor| sensor.read()),
                );
                device.add(
                    "fan:0:speed",
                    "GPU fan speed",
                    "RPM",
                    metrics.fan_speed().map(f64::from).map_err(adlx_issue),
                );
                device.add(
                    "core_clock",
                    "GPU clock",
                    "MHz",
                    metrics.clock_speed().map(f64::from).map_err(adlx_issue),
                );
                device.add(
                    "memory_clock",
                    "VRAM clock",
                    "MHz",
                    metrics
                        .vram_clock_speed()
                        .map(f64::from)
                        .map_err(adlx_issue),
                );
                device.add(
                    "memory_used",
                    "VRAM used",
                    "B",
                    metrics
                        .vram()
                        .map(|mb| f64::from(mb) * 1_048_576.0)
                        .map_err(adlx_issue),
                );
                device.add(
                    "core_voltage",
                    "GPU voltage",
                    "V",
                    metrics
                        .voltage()
                        .map(|mv| f64::from(mv) / 1000.0)
                        .map_err(adlx_issue),
                );
                device.add(
                    "chip_power",
                    "GPU power",
                    "W",
                    metrics.power().map_err(adlx_issue),
                );
                device.add(
                    "board_power",
                    "GPU board power",
                    "W",
                    metrics.total_board_power().map_err(adlx_issue),
                );
            }
            Err(error) => device.add("usage", "GPU utilization", "%", Err(adlx_issue(error))),
        }
        devices.push(device);
    }
    if devices.is_empty() {
        return Err(Issue::new(Status::Unsupported, "no Radeon adapters found"));
    }
    Ok(devices)
}

pub fn cpu() -> Result<Vec<Device>, Issue> {
    let program_files = env::var_os("ProgramW6432")
        .or_else(|| env::var_os("ProgramFiles"))
        .ok_or_else(|| Issue::unavailable("Program Files unavailable"))?;
    let cli = PathBuf::from(program_files)
        .join(r"AMD\RyzenMasterSDK\AMDRyzenMasterCLI\bin-prebuilt\AMDRyzenMasterCLI.exe");
    if !cli.is_file() {
        return Err(Issue::unavailable("AMD SDK sample CLI is not installed"));
    }

    // The sample can display a modal error when launched without elevation.
    // SAFETY: IsUserAnAdmin takes no arguments and returns a Win32 BOOL.
    if unsafe { IsUserAnAdmin() } == 0 {
        return Err(Issue::new(
            Status::PermissionDenied,
            "AMD SDK sample CLI requires an administrator terminal",
        ));
    }

    let output = read_pm_table(cli).map_err(|error| Issue::unavailable(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.contains("User is not admin") || stderr.contains("User is not admin") {
        return Err(Issue::new(
            Status::PermissionDenied,
            "AMD SDK sample CLI requires an administrator terminal",
        ));
    }
    if !output.status.success() {
        return Err(Issue::unavailable(format!(
            "AMD CLI failed ({}) {}",
            output.status,
            stderr.trim()
        )));
    }

    let fields = [
        ("GetCurrentTemperature", "CPU temperature", "Celsius", " C"),
        ("PPT Current Value", "CPU socket power (PPT)", "W", " W"),
        ("VDDCR_CPU_POWER", "CPU core rail power", "W", " W"),
        ("VDDCR_SOC_POWER", "CPU SoC rail power", "W", " W"),
        ("GetCPUTelemetryVoltage", "CPU telemetry voltage", "V", " V"),
        (
            "GetAverageCoreVoltage",
            "CPU average core voltage",
            "V",
            " V",
        ),
        (
            "GetSOCTelemetryVoltage",
            "CPU SoC telemetry voltage",
            "V",
            " V",
        ),
        ("EDC Current Value", "CPU EDC", "A", " A"),
        ("TDC Current Value", "CPU TDC", "A", " A"),
        ("Fabric Clock Frequency", "Fabric clock", "MHz", " MHz"),
        (
            "P0 Memory Clock Frequency",
            "Memory clock (P0)",
            "MHz",
            " MHz",
        ),
        (
            "P0 UCLK Frequency",
            "Memory controller clock (P0)",
            "MHz",
            " MHz",
        ),
    ];
    let mut device = Device::new("cpu:system", "AMD Ryzen processor", "cpu");
    let mut found = false;
    for (key, label, unit, suffix) in fields {
        let value = stdout.lines().find_map(|line| {
            let (left, right) = line.split_once("...")?;
            let right = right.trim_start_matches('.').trim();
            let tokens = if left.trim() == "GetPMTableData" {
                let (field, value) = right.split_once(':')?;
                (field.trim() == key).then_some(value)?
            } else {
                (left.trim() == key).then_some(right)?
            };
            number_with_unit(tokens, unit)
        });
        found |= value.is_some();
        let id = match key {
            "GetCurrentTemperature" => "temperature",
            "PPT Current Value" => "socket_power",
            "VDDCR_CPU_POWER" => "core_rail_power",
            "VDDCR_SOC_POWER" => "soc_rail_power",
            "GetCPUTelemetryVoltage" => "telemetry_voltage",
            "GetAverageCoreVoltage" => "average_core_voltage",
            "GetSOCTelemetryVoltage" => "soc_telemetry_voltage",
            "EDC Current Value" => "edc",
            "TDC Current Value" => "tdc",
            "Fabric Clock Frequency" => "fabric_clock",
            "P0 Memory Clock Frequency" => "memory_clock",
            _ => "memory_controller_clock",
        };
        let output_unit = if suffix == " C" { "°C" } else { suffix.trim() };
        device.add(
            id,
            label,
            output_unit,
            value.ok_or_else(|| Issue::unavailable("not reported by AMD SDK")),
        );
    }
    for line in stdout.lines() {
        let Some((kind, core_and_value)) = line.split_once(':') else {
            continue;
        };
        let label = match kind.trim() {
            "GetCurrentFrequency Core" => "clock",
            "GetEffectiveFrequency Core" => "effective clock",
            _ => continue,
        };
        let Some((core, value)) = core_and_value.split_once("...") else {
            continue;
        };
        let Ok(core) = core.trim().parse::<u32>() else {
            continue;
        };
        if let Some(mhz) = number_with_unit(value.trim_start_matches('.'), "MHz") {
            found = true;
            device.add(
                format!("core:{core}:{}", label.replace(' ', "_")),
                format!("CPU core {core} {label}"),
                "MHz",
                Ok(mhz),
            );
        }
    }
    if !found {
        return Err(Issue::unavailable(
            "AMD CLI returned no recognized CPU telemetry",
        ));
    }
    Ok(vec![device])
}

fn number_with_unit(value: &str, unit: &str) -> Option<f64> {
    let mut parts = value.split_whitespace();
    let number = parts.next()?.parse::<f64>().ok()?;
    let reported_unit = parts.next()?.split('-').next()?;
    (number.is_finite() && reported_unit == unit).then_some(number)
}

fn read_pm_table(cli: PathBuf) -> io::Result<Output> {
    let mut child = Command::new(cli)
        .args(["--api", "GetPMTableData"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("AMD SDK stdout is piped");
    let mut stderr = child.stderr.take().expect("AMD SDK stderr is piped");

    thread::scope(|scope| {
        // Drain both pipes while the SDK runs, including output larger than a pipe buffer.
        let read_out = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let read_err = scope.spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        // SAFETY: the child owns a live process handle for the duration of the wait.
        let wait = unsafe { WaitForSingleObject(child.as_raw_handle(), 8_000) };
        let status = match wait {
            WAIT_OBJECT_0 => child.wait(),
            WAIT_TIMEOUT => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "AMD SDK did not respond within 8 seconds",
            )),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            code => Err(io::Error::other(format!(
                "unexpected AMD SDK wait result: {code}"
            ))),
        };
        let status = status.inspect_err(|_| {
            let _ = child.kill();
            let _ = child.wait();
        })?;
        Ok(Output {
            status,
            stdout: read_out.join().expect("stdout reader panicked")?,
            stderr: read_err.join().expect("stderr reader panicked")?,
        })
    })
}
