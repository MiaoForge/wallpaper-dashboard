use std::{
    cell::RefCell,
    thread,
    time::{Duration, Instant},
};

use crate::telemetry::{Device, Issue};
use sysinfo::{Disks, MINIMUM_CPU_UPDATE_INTERVAL, Networks, System};

struct Sampler {
    system: System,
    networks: Result<Networks, sysinfo::Error>,
    network_sampled_at: Instant,
}

impl Sampler {
    fn new() -> Result<Self, Issue> {
        let mut system = System::new().map_err(|error| Issue::unavailable(error.to_string()))?;
        system.refresh_cpu_usage();
        let networks = Networks::new_with_refreshed_list();
        let network_sampled_at = Instant::now();
        thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_secs(1)));
        Ok(Self {
            system,
            networks,
            network_sampled_at,
        })
    }
}

thread_local! {
    static LIVE_SAMPLER: RefCell<Option<Sampler>> = const { RefCell::new(None) };
}

pub fn sample() -> Result<Vec<Device>, Issue> {
    Ok(Sampler::new()?.collect())
}

pub(crate) fn sample_live() -> Result<Vec<Device>, Issue> {
    LIVE_SAMPLER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(Sampler::new()?);
        }
        Ok(slot.as_mut().expect("sampler initialized").collect())
    })
}

impl Sampler {
    fn collect(&mut self) -> Vec<Device> {
        let system = &mut self.system;
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

        let needs_network_baseline = self.networks.is_err();
        if needs_network_baseline {
            self.networks = Networks::new_with_refreshed_list();
        }
        match &mut self.networks {
            Ok(networks) if !needs_network_baseline => {
                networks.refresh(true);
                let elapsed = self.network_sampled_at.elapsed().as_secs_f64();
                for (name, network) in &*networks {
                    let name = name.to_string_lossy();
                    let mut device =
                        Device::new(format!("network:{name}"), name.as_ref(), "network");
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
            Ok(_) => {}
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
        self.network_sampled_at = Instant::now();
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
        devices
    }
}
