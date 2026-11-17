use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    Unsupported,
    PermissionDenied,
    Unavailable,
}

#[derive(Clone, Serialize)]
pub struct Issue {
    pub status: Status,
    pub detail: String,
}

impl Issue {
    pub fn new(status: Status, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(Status::Unavailable, detail)
    }
}

#[derive(Serialize)]
pub struct Sensor {
    pub id: String,
    pub label: String,
    pub unit: &'static str,
    pub value: Option<f64>,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seen_spinning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<&'static str>,
    pub sensors: Vec<Sensor>,
}

impl Device {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: &'static str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            volume_label: None,
            network_type: None,
            connection: None,
            sensors: Vec::new(),
        }
    }

    pub fn add(
        &mut self,
        id: impl Into<String>,
        label: impl Into<String>,
        unit: &'static str,
        result: Result<f64, Issue>,
    ) {
        let (value, status, detail) = match result {
            Ok(value) if value.is_finite() => (Some(value), Status::Available, None),
            Ok(_) => (None, Status::Unavailable, Some("non-finite reading".into())),
            Err(issue) => (None, issue.status, Some(issue.detail)),
        };
        self.sensors.push(Sensor {
            id: id.into(),
            label: label.into(),
            unit,
            value,
            status,
            seen_spinning: None,
            detail,
        });
    }

    pub(crate) fn add_fan(
        &mut self,
        id: String,
        label: String,
        rpm: Result<f64, Issue>,
        seen_spinning: bool,
    ) {
        self.add(id, label, "RPM", rpm);
        self.sensors
            .last_mut()
            .expect("just added fan")
            .seen_spinning = Some(seen_spinning);
    }
}

#[derive(Serialize)]
pub struct Backend {
    source: &'static str,
    sampled_at_unix_ms: u64,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    devices: Vec<Device>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_duration_ms: Option<u64>,
}

impl Backend {
    pub fn new(source: &'static str, result: Result<Vec<Device>, Issue>) -> Self {
        let (status, detail, devices) = match result {
            Ok(devices) => (Status::Available, None, devices),
            Err(issue) => (issue.status, Some(issue.detail), Vec::new()),
        };
        let sampled_at_unix_ms = unix_ms();
        Self {
            source,
            status,
            detail,
            devices,
            sampled_at_unix_ms,
            last_error: None,
            sample_duration_ms: None,
        }
    }
    pub fn record(
        &mut self,
        result: Result<Vec<Device>, Issue>,
        attempted_at: u64,
        duration_ms: u64,
        retention_ms: u64,
    ) {
        self.sample_duration_ms = Some(duration_ms);
        if let Err(issue) = &result
            && issue.status == Status::Unavailable
            && self.status == Status::Available
            && attempted_at >= self.sampled_at_unix_ms
            && attempted_at - self.sampled_at_unix_ms <= retention_ms
        {
            self.last_error = result.err();
            return;
        }

        let mut new = Self::new(self.source, result);
        new.sampled_at_unix_ms = attempted_at;
        new.sample_duration_ms = Some(duration_ms);
        *self = new;
    }
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as u64)
}
pub fn print_devices(devices: &[Device]) {
    for device in devices {
        println!("  {}", device.name);
        for sensor in &device.sensors {
            match sensor.value {
                Some(value) => println!("    {}: {value} {}", sensor.label, sensor.unit),
                None => println!(
                    "    {}: unavailable ({})",
                    sensor.label,
                    sensor.detail.as_deref().unwrap_or("no reading")
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_failure_preserves_only_a_recent_success_and_then_expires() {
        let mut old = Device::new("motherboard:system", "Board", "motherboard");
        old.add("fan:1:speed", "Fan 1", "RPM", Ok(0.0));
        let mut state = Backend::new("msi_center", Ok(vec![]));
        state.record(Ok(vec![old]), 1000, 5, 7000);
        state.record(Err(Issue::unavailable("busy")), 3000, 21, 7000);
        assert_eq!(state.sampled_at_unix_ms, 1000);
        assert_eq!(state.status, Status::Available);
        assert_eq!(state.last_error.as_ref().unwrap().detail, "busy");
        assert_eq!(state.sample_duration_ms, Some(21));
        assert_eq!(state.devices[0].sensors[0].value, Some(0.0));

        state.record(Err(Issue::unavailable("busy")), 8001, 21, 7000);
        assert_eq!(state.status, Status::Unavailable);
        assert!(state.devices.is_empty());
        assert!(state.last_error.is_none());

        state.record(Ok(vec![]), 8500, 3, 7000);
        assert_eq!(state.status, Status::Available);
        assert_eq!(state.sampled_at_unix_ms, 8500);
    }

    #[test]
    fn permission_loss_invalidates_previous_readings_immediately() {
        let mut state = Backend::new("msi_center", Ok(vec![]));
        state.record(Ok(vec![]), 1000, 5, 7000);
        state.record(
            Err(Issue::new(Status::PermissionDenied, "access denied")),
            2000,
            1,
            7000,
        );
        assert_eq!(state.status, Status::PermissionDenied);
        assert!(state.last_error.is_none());
        assert!(state.devices.is_empty());
    }
}
