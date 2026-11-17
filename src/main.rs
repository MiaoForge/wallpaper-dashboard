use sensor_probe::{
    amd, battery, fans, frames, hid_battery, msi, nvidia, service, shortcuts, telemetry, windows,
};
use std::{io, process::Command};
use telemetry::{Backend, Device, Issue, print_devices};

fn print_result(result: Result<Vec<Device>, Issue>) {
    match result {
        Ok(devices) => print_devices(&devices),
        Err(issue) => println!("  Unavailable ({})", issue.detail),
    }
}

fn json_snapshot() -> Result<(), serde_json::Error> {
    let backends = [
        Backend::new("windows", windows::sample()),
        Backend::new("amd_adlx", amd::gpu()),
        Backend::new("amd_ryzen_sdk", amd::cpu()),
        Backend::new("msi_center", msi::sample()),
        Backend::new("windows_battery", battery::sample()),
        Backend::new("bluetooth_devices", battery::sample_bluetooth()),
        Backend::new("hid_battery", hid_battery::sample()),
        Backend::new("nvidia_nvml", nvidia::sample()),
    ];
    let snapshot = serde_json::json!({ "schema_version": 1, "backends": backends });
    serde_json::to_writer_pretty(io::stdout().lock(), &snapshot)?;
    println!();
    Ok(())
}

fn windows_inventory() {
    // Win32_Fan can enumerate firmware-provided fan inventory. DesiredSpeed is
    // a target, so never report it as measured RPM.
    let script = r#"
        [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
        try {
            $board = Get-CimInstance Win32_BaseBoard -ErrorAction Stop
            $bios = Get-CimInstance Win32_BIOS -ErrorAction Stop
            "Mainboard: $($board.Manufacturer) $($board.Product), BIOS $($bios.SMBIOSBIOSVersion)"
            foreach ($module in Get-CimInstance Win32_PhysicalMemory -ErrorAction Stop) {
                $capacity = [Math]::Round($module.Capacity / 1GB, 1)
                "DIMM $($module.DeviceLocator): $capacity GiB, part $(([string]$module.PartNumber).Trim()), configured $($module.ConfiguredClockSpeed) MT/s (SMBIOS)"
            }
        } catch { "System inventory: unavailable ($($_.Exception.Message))" }
        try {
            foreach ($disk in Get-PhysicalDisk -ErrorAction Stop) {
                try {
                    $health = $disk | Get-StorageReliabilityCounter -ErrorAction Stop
                    $temperature = if ($health.Temperature -gt 0) { "$($health.Temperature) C" } else { 'unavailable' }
                    $hours = if ($null -ne $health.PowerOnHours) { "$($health.PowerOnHours) h" } else { 'unavailable' }
                    "$($disk.FriendlyName) ($($disk.BusType)): health $($disk.HealthStatus), temperature $temperature, power-on time $hours"
                } catch {
                    "$($disk.FriendlyName) ($($disk.BusType)): health $($disk.HealthStatus), reliability counters unavailable ($($_.Exception.Message))"
                }
            }
        } catch { "Physical disks: unavailable ($($_.Exception.Message))" }
        try {
            $fans = @(Get-CimInstance Win32_Fan -ErrorAction Stop)
            "Win32_Fan inventory: $($fans.Count) entries (no measured RPM field)"
        } catch { "Win32_Fan inventory: unavailable ($($_.Exception.Message))" }
        try {
            $zones = @(Get-CimInstance -Namespace root/wmi -ClassName MSAcpi_ThermalZoneTemperature -ErrorAction Stop)
            "ACPI thermal zones: $($zones.Count) entries"
            foreach ($zone in $zones) {
                $c = [Math]::Round($zone.CurrentTemperature / 10.0 - 273.15, 1)
                "ACPI zone $($zone.InstanceName): $c C"
            }
        } catch { "ACPI thermal zones: unavailable ($($_.Exception.Message))" }
    "#;
    match Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
    {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("  {line}");
            }
        }
        Ok(output) => println!("  Firmware inventory query failed ({})", output.status),
        Err(error) => println!("  Firmware inventory query failed ({error})"),
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [flag, folder] = args.as_slice()
        && flag == "--pair-shortcuts"
    {
        return match shortcuts::pair(std::path::Path::new(folder)) {
            Ok(()) => {
                println!("Wallpaper shortcut pairing created.");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Could not pair the wallpaper: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let [flag, path] = args.as_slice()
        && flag == "--track-foreground"
    {
        return match frames::automatic(std::path::Path::new(path)) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Could not enable FPS tracking: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.as_slice() == ["--serve"] {
        return match service::run() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Could not serve dashboard: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.as_slice() == ["--json"] {
        return match json_snapshot() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Could not write snapshot: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let [flag] = args.as_slice()
        && flag == "--msi-fans"
    {
        println!("Motherboard fans (MSI Center)");
        return match msi::sample() {
            Ok(devices) => {
                print_devices(&devices);
                std::process::ExitCode::SUCCESS
            }
            Err(issue) => {
                eprintln!("  Motherboard fans unavailable ({})", issue.detail);
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let [flag, module] = args.as_slice()
        && flag == "--lpcio"
    {
        println!("Motherboard fans (PawnIO / LpcIO)");
        return match fans::report(std::path::Path::new(module)) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("  Motherboard fans unavailable ({error})");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let [flag, presentmon, pid] = args.as_slice()
        && (flag == "--presentmon" || flag == "--track-fps")
        && let Some(pid) = pid.to_str().and_then(|value| value.parse::<u32>().ok())
        && pid != 0
    {
        println!("Frame capture (PresentMon, PID {pid})");
        let path = std::path::Path::new(presentmon);
        let result = if flag == "--track-fps" {
            frames::track(path, pid)
        } else {
            frames::report(path, pid)
        };
        return match result {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("  Frame metrics unavailable ({error})");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if !args.is_empty() {
        eprintln!(
            "Usage: sensor-probe [--track-foreground PRESENTMON_EXE | --pair-shortcuts WALLPAPER_FOLDER | --serve | --json | --msi-fans | --lpcio PATH_TO_LpcIO.bin | --presentmon PATH_TO_EXE PID | --track-fps PATH_TO_EXE PID]"
        );
        return std::process::ExitCode::FAILURE;
    }
    println!("Windows");
    print_result(windows::sample());
    windows_inventory();
    println!("AMD Radeon (ADLX)");
    print_result(amd::gpu());
    println!("AMD Ryzen Master SDK");
    print_result(amd::cpu());
    std::process::ExitCode::SUCCESS
}
