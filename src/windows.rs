use std::{
    thread,
    time::{Duration, Instant},
};

use crate::telemetry::{Device, Issue};
use sysinfo::{Disks, MINIMUM_CPU_UPDATE_INTERVAL, Networks, System};

pub fn sample() -> Result<Vec<Device>, Issue> {
    let mut system = System::new().map_err(|error| Issue::unavailable(error.to_string()))?;
    let networks = Networks::new_with_refreshed_list();
    system.refresh_cpu_usage();
    let started = Instant::now();
    thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_secs(1)));
    system.refresh_cpu_usage();
    system.refresh_memory();

    let cpu_name = system
        .cpus()
        .first()
        .map_or("CPU", |cpu| cpu.brand().trim());
    let mut cpu = Device::new("cpu:system", cpu_name, "cpu");
    if !system.cpus().is_empty() {
        cpu.add(
            "usage",
            "CPU usage",
            "%",
            Ok(system.global_cpu_usage().into()),
        );
        for (index, logical) in system.cpus().iter().enumerate() {
            cpu.add(
                format!("logical:{index}:usage"),
                format!("Logical CPU {index} usage"),
                "%",
                Ok(logical.usage().into()),
            );
        }
    }
    let mut ram = Device::new("memory:system", "System memory", "memory");
    ram.add("used", "Used RAM", "B", Ok(system.used_memory() as f64));
    ram.add("total", "Total RAM", "B", Ok(system.total_memory() as f64));
    if system.total_memory() > 0 {
        ram.add(
            "usage",
            "RAM usage",
            "%",
            Ok(100.0 * system.used_memory() as f64 / system.total_memory() as f64),
        );
    }
    let mut devices = vec![cpu, ram];

    match networks {
        Ok(mut networks) => {
            networks.refresh(false);
            let elapsed = started.elapsed().as_secs_f64();
            for (name, network) in &networks {
                let name = name.to_string_lossy();
                let mut device = Device::new(format!("network:{name}"), name.as_ref(), "network");
                device.add(
                    "receive_rate",
                    "Receive rate",
                    "B/s",
                    Ok(network.received() as f64 / elapsed),
                );
                device.add(
                    "transmit_rate",
                    "Transmit rate",
                    "B/s",
                    Ok(network.transmitted() as f64 / elapsed),
                );
                devices.push(device);
            }
        }
        Err(error) => {
            let mut device = Device::new("network:system", "Network interfaces", "network");
            device.add(
                "receive_rate",
                "Receive rate",
                "B/s",
                Err(Issue::unavailable(error.to_string())),
            );
            devices.push(device);
        }
    }
    match Disks::new_with_refreshed_list() {
        Ok(disks) => {
            for disk in &disks {
                let name = disk.mount_point().to_string_lossy();
                let mut device = Device::new(format!("volume:{name}"), name.as_ref(), "volume");
                let label = disk.name().to_string_lossy();
                if !label.trim().is_empty() {
                    device.volume_label = Some(label.into_owned());
                }
                device.add(
                    "free",
                    "Free disk space",
                    "B",
                    Ok(disk.available_space() as f64),
                );
                device.add("total", "Disk capacity", "B", Ok(disk.total_space() as f64));
                devices.push(device);
            }
        }
        Err(error) => {
            let mut device = Device::new("volume:system", "Logical volumes", "volume");
            device.add(
                "free",
                "Free disk space",
                "B",
                Err(Issue::unavailable(error.to_string())),
            );
            devices.push(device);
        }
    }
    Ok(devices)
}
