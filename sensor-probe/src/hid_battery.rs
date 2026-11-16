use std::{
    fs::OpenOptions,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    time::{Duration, Instant},
};

use hidapi::{DeviceInfo, HidApi};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_GetCaps,
};

use crate::telemetry::{Device, Issue};

const CONTROL_PAGE: u16 = 0xff02;
const CONTROL_USAGE: u16 = 2;
const MCHOSE_REQUEST: [u8; 20] = [
    0x13, 0x4a, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const NEARLINK_REQUEST: [u8; 17] = [0x08, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x49];

struct BatteryReport {
    level: u8,
    charging: Option<bool>,
}

struct Receiver {
    id: (u16, u16),
    name: &'static str,
    request: &'static [u8],
    parse: fn(&[u8]) -> Option<BatteryReport>,
}

const RECEIVERS: [Receiver; 2] = [
    Receiver {
        id: (0x41e4, 0x2001),
        name: "MCHOSE 无线键盘",
        request: &MCHOSE_REQUEST,
        parse: parse_mchose,
    },
    Receiver {
        id: (0x373b, 0x10c9),
        name: "NearLink 无线鼠标",
        request: &NEARLINK_REQUEST,
        parse: parse_nearlink,
    },
];

fn parse_mchose(reply: &[u8]) -> Option<BatteryReport> {
    if !reply.starts_with(&[0x13, 0x4a, 0x01, 0x00]) {
        return None;
    }
    let level = *reply.get(5)?;
    (level <= 100).then_some(BatteryReport {
        level,
        charging: None,
    })
}

fn parse_nearlink(reply: &[u8]) -> Option<BatteryReport> {
    let [data @ .., checksum] = reply.get(..NEARLINK_REQUEST.len())? else {
        return None;
    };
    let expected = data
        .iter()
        .fold(0x55u8, |sum, value| sum.wrapping_sub(*value));
    if !data.starts_with(&NEARLINK_REQUEST[..2]) || *checksum != expected || data[6] > 100 {
        return None;
    }
    Some(BatteryReport {
        level: data[6],
        charging: Some(data[7] != 0),
    })
}

fn output_report_length(info: &DeviceInfo) -> Option<usize> {
    // Metadata-only access to the collection, including when another app is reading it.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(3)
        .open(info.path().to_str().ok()?)
        .ok()?;
    let mut preparsed = 0;
    let mut caps = HIDP_CAPS::default();
    // SAFETY: The file remains open, and the output pointers refer to writable storage.
    unsafe {
        if !HidD_GetPreparsedData(file.as_raw_handle(), &mut preparsed) {
            return None;
        }
        let result = HidP_GetCaps(preparsed, &mut caps);
        HidD_FreePreparsedData(preparsed);
        (result == HIDP_STATUS_SUCCESS).then_some(usize::from(caps.OutputReportByteLength))
    }
}

fn query(api: &HidApi, info: &DeviceInfo, receiver: &Receiver) -> Result<BatteryReport, Issue> {
    let error =
        |detail: &dyn std::fmt::Display| Issue::unavailable(format!("{}: {detail}", receiver.name));
    let hid = info.open_device(api).map_err(|cause| error(&cause))?;
    let mut reply = [0; 64];
    for _ in 0..4 {
        if hid
            .read_timeout(&mut reply, 0)
            .map_err(|cause| error(&cause))?
            == 0
        {
            break;
        }
    }
    let written = hid.write(receiver.request).map_err(|cause| error(&cause))?;
    if written != receiver.request.len() {
        return Err(error(&"incomplete battery query"));
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let count = hid
            .read_timeout(&mut reply, remaining.as_millis() as i32)
            .map_err(|cause| error(&cause))?;
        if count == 0 {
            break;
        }
        if let Some(report) = (receiver.parse)(&reply[..count]) {
            return Ok(report);
        }
    }
    Err(error(&"no valid battery reply"))
}

pub fn sample() -> Result<Vec<Device>, Issue> {
    use sha2::{Digest, Sha256};
    let api = HidApi::new().map_err(|cause| Issue::unavailable(cause.to_string()))?;
    let mut devices = Vec::new();
    let mut last_issue = None;
    for receiver in &RECEIVERS {
        for info in api.device_list().filter(|info| {
            (info.vendor_id(), info.product_id()) == receiver.id
                && (info.usage_page(), info.usage()) == (CONTROL_PAGE, CONTROL_USAGE)
                && output_report_length(info) == Some(receiver.request.len())
        }) {
            match query(&api, info, receiver) {
                Ok(report) => {
                    let id = hex::encode(Sha256::digest(info.path().to_bytes()));
                    let mut device =
                        Device::new(format!("hid-battery:{id}"), receiver.name, "battery");
                    device.add(
                        "charge",
                        "Receiver-reported battery level",
                        "%",
                        Ok(f64::from(report.level)),
                    );
                    if let Some(charging) = report.charging {
                        device.add(
                            "external_power",
                            "On external power",
                            "",
                            Ok(f64::from(u8::from(charging))),
                        );
                    }
                    devices.push(device);
                }
                Err(issue) => last_issue = Some(issue),
            }
        }
    }
    if devices.is_empty()
        && let Some(issue) = last_issue
    {
        return Err(issue);
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mchose_requires_a_battery_reply_and_valid_percentage() {
        assert_eq!(parse_mchose(&[0x13, 0x4a, 1, 0, 2, 52]).unwrap().level, 52);
        assert!(parse_mchose(&[0x13, 0x4a, 1, 0, 2, 101]).is_none());
        assert!(parse_mchose(&[0x13, 0x4b, 1, 0, 2, 52]).is_none());
    }

    #[test]
    fn nearlink_requires_the_power_command_and_matching_checksum() {
        let mut reply = [
            0x08, 0x04, 0, 0, 0, 2, 0x55, 0, 0x0f, 0xac, 0, 0, 0, 0, 0, 0, 0x37,
        ];
        assert_eq!(parse_nearlink(&reply).unwrap().level, 85);
        assert!(parse_nearlink(&reply[..16]).is_none());
        reply[6] -= 1;
        assert!(parse_nearlink(&reply).is_none());
    }
}
