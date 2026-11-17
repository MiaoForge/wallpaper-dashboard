use std::{collections::BTreeMap, ptr};

use sha2::{Digest, Sha256};
use windows::{
    Devices::Enumeration::{DeviceInformation, DeviceInformationKind},
    core::HSTRING,
};

use windows_sys::{
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                CM_GETIDLIST_FILTER_PRESENT, CM_Get_Child, CM_Get_DevNode_PropertyW,
                CM_Get_DevNode_Status, CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW,
                CM_Get_Sibling, CM_Locate_DevNodeW, CR_SUCCESS, DN_HAS_PROBLEM, DN_STARTED,
            },
            Properties::{
                DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_Class,
                DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPKEY_Device_Service,
                DEVPROP_TYPE_BYTE, DEVPROP_TYPE_STRING,
            },
        },
        Foundation::DEVPROPKEY,
    },
    core::GUID,
};

use crate::telemetry::{Device, Issue};

const BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

const AEP_ID: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x3b2ce006_5e61_4fde_bab8_9b8aac9b26df),
    pid: 8,
};

fn device_key(id: &str) -> String {
    format!("peripheral:{}", hex::encode(Sha256::digest(id)))
}

fn connected_bluetooth() -> windows::core::Result<BTreeMap<String, Device>> {
    let filter = HSTRING::from(concat!(
        "System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True AND ",
        "System.Devices.Aep.IsConnected:=System.StructuredQueryType.Boolean#True AND (",
        r#"System.Devices.Aep.ProtocolId:="{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}" OR "#,
        r#"System.Devices.Aep.ProtocolId:="{bb7bb05e-5972-42b5-94fc-76eaa7084d49}")"#,
    ));
    let endpoints = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &filter,
        None,
        DeviceInformationKind::AssociationEndpoint,
    )?
    .join()?;
    let mut devices = BTreeMap::new();
    for endpoint in endpoints {
        let label = endpoint.Name()?.to_string();
        if !label.trim().is_empty() {
            let id = endpoint.Id()?.to_string().to_ascii_lowercase();
            let mut device = Device::new(device_key(&id), label, "peripheral");
            device.connection = Some("bluetooth");
            devices.insert(id, device);
        }
    }
    Ok(devices)
}

fn usb_device(node: u32, id: &str) -> Option<Device> {
    let hardware = id.strip_prefix("USB\\")?.split('\\').next()?;
    if !hardware.starts_with("VID_")
        || hardware.len() != "VID_0000&PID_0000".len()
        || &hardware.as_bytes()[8..13] != b"&PID_"
    {
        return None;
    }
    let mut status = 0;
    let mut problem = 0;
    // SAFETY: Both output pointers are writable and node is a live instance handle.
    if unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, node, 0) } != CR_SUCCESS
        || status & (DN_STARTED | DN_HAS_PROBLEM) != DN_STARTED
        || name(node, &DEVPKEY_Device_Service).is_some_and(|service| {
            service.eq_ignore_ascii_case("usbhub") || service.eq_ignore_ascii_case("usbhub3")
        })
    {
        return None;
    }

    let class = name(node, &DEVPKEY_Device_Class).unwrap_or_default();
    if class.eq_ignore_ascii_case("Bluetooth") {
        return None;
    }
    let mut child_label = None;
    let mut child = 0;
    // SAFETY: The output pointer is writable. Each successful call yields a child handle.
    let mut child_state = unsafe { CM_Get_Child(&mut child, node, 0) };
    while child_state == CR_SUCCESS {
        if let Some(class) = name(child, &DEVPKEY_Device_Class) {
            if class.eq_ignore_ascii_case("Bluetooth") {
                return None;
            }
            if child_label.is_none() && matches!(class.as_str(), "MEDIA" | "Camera" | "Image") {
                child_label = name(child, &DEVPKEY_Device_FriendlyName)
                    .or_else(|| name(child, &DEVPKEY_Device_DeviceDesc));
            }
        }
        // SAFETY: child is a valid instance handle and the output pointer is writable.
        child_state = unsafe { CM_Get_Sibling(&mut child, child, 0) };
    }

    let is_gamepad = class.eq_ignore_ascii_case("XnaComposite");
    let label = name(node, &DEVPKEY_Device_BusReportedDeviceDesc)
        .filter(|label| !label.eq_ignore_ascii_case("USB Composite Device"))
        .or(child_label)
        .or_else(|| name(node, &DEVPKEY_Device_FriendlyName))
        .or_else(|| name(node, &DEVPKEY_Device_DeviceDesc))?;
    let lower_label = label.to_lowercase();
    let named_receiver = ["receiver", "dongle", "接收器"]
        .iter()
        .any(|term| lower_label.contains(term));
    let is_receiver =
        named_receiver || matches!(hardware, "VID_41E4&PID_2001" | "VID_373B&PID_10C9");
    let label = if is_receiver && !named_receiver {
        format!("{label} 接收器")
    } else {
        label
    };
    let mut device = Device::new(
        device_key(id),
        label,
        if is_gamepad { "gamepad" } else { "peripheral" },
    );
    device.connection = Some(if is_receiver {
        "usb_receiver"
    } else if is_gamepad {
        "usb_interface"
    } else {
        "usb"
    });
    Some(device)
}

fn property(node: u32, key: &DEVPROPKEY, output: &mut [u8]) -> Option<(u32, usize)> {
    let mut kind = 0;
    let mut length = output.len() as u32;
    // SAFETY: The key, type, size and output array are valid for the duration of the call.
    let result = unsafe {
        CM_Get_DevNode_PropertyW(node, key, &mut kind, output.as_mut_ptr(), &mut length, 0)
    };
    (result == CR_SUCCESS && length as usize <= output.len()).then_some((kind, length as usize))
}

fn name(node: u32, key: &DEVPROPKEY) -> Option<String> {
    let mut bytes = [0; 1024];
    let (kind, len) = property(node, key, &mut bytes)?;
    (kind == DEVPROP_TYPE_STRING)
        .then(|| {
            let units: Vec<_> = bytes[..len]
                .as_chunks::<2>()
                .0
                .iter()
                .copied()
                .map(u16::from_le_bytes)
                .take_while(|unit| *unit != 0)
                .collect();
            String::from_utf16_lossy(&units)
        })
        .filter(|value| !value.trim().is_empty())
}

fn present_nodes() -> Result<Vec<(String, u32)>, Issue> {
    let mut count = 0;
    // SAFETY: The null filter asks for all present nodes; count is writable.
    let status = unsafe {
        CM_Get_Device_ID_List_SizeW(&mut count, ptr::null(), CM_GETIDLIST_FILTER_PRESENT)
    };
    if status != CR_SUCCESS {
        return Err(Issue::unavailable(format!(
            "Windows PnP enumeration failed ({status})"
        )));
    }
    let mut buffer = vec![0u16; count as usize];
    // SAFETY: buffer holds count writable UTF-16 units.
    let status = unsafe {
        CM_Get_Device_ID_ListW(
            ptr::null(),
            buffer.as_mut_ptr(),
            count,
            CM_GETIDLIST_FILTER_PRESENT,
        )
    };
    if status != CR_SUCCESS {
        return Err(Issue::unavailable(format!(
            "Windows PnP list changed during sampling ({status})"
        )));
    }

    let mut nodes = Vec::new();
    let mut offset = 0;
    while let Some(entries) = buffer.get(offset..) {
        let Some(end) = entries.iter().position(|unit| *unit == 0) else {
            break;
        };
        if end == 0 {
            break;
        }
        let id = String::from_utf16_lossy(&entries[..end]);
        let mut node = 0;
        // SAFETY: entries starts with a NUL-terminated instance ID in the list.
        let found = unsafe { CM_Locate_DevNodeW(&mut node, entries.as_ptr(), 0) };
        offset += end + 1;
        if found == CR_SUCCESS {
            nodes.push((id, node));
        }
    }
    Ok(nodes)
}

pub fn sample() -> Result<Vec<Device>, Issue> {
    let mut devices = Vec::new();
    for (id, node) in present_nodes()? {
        if id.starts_with("USB\\") {
            devices.extend(usb_device(node, &id));
        } else if let Some(percent) = charge(node)
            && !id.starts_with("BTH")
            && name(node, &AEP_ID).is_none()
            && let Some(label) = name(node, &DEVPKEY_Device_FriendlyName)
                .or_else(|| name(node, &DEVPKEY_Device_DeviceDesc))
        {
            let mut device = Device::new(device_key(&id), label, "battery");
            add_charge(&mut device, percent);
            devices.push(device);
        }
    }
    devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(devices)
}

pub fn sample_bluetooth() -> Result<Vec<Device>, Issue> {
    let mut devices = connected_bluetooth()
        .map_err(|error| Issue::unavailable(format!("Bluetooth endpoints: {error}")))?;
    if !devices.is_empty() {
        for (_, node) in present_nodes()? {
            if let Some(percent) = charge(node)
                && let Some(aep) = name(node, &AEP_ID)
                && let Some(device) = devices.get_mut(&aep.to_ascii_lowercase())
                && device.sensors.is_empty()
            {
                device.kind = "battery";
                add_charge(device, percent);
            }
        }
    }
    Ok(devices.into_values().collect())
}

fn charge(node: u32) -> Option<u8> {
    let mut percent = [0];
    (property(node, &BLUETOOTH_BATTERY, &mut percent) == Some((DEVPROP_TYPE_BYTE, 1))
        && percent[0] <= 100)
        .then_some(percent[0])
}

fn add_charge(device: &mut Device, percent: u8) {
    device.add(
        "charge",
        "Windows-reported battery level",
        "%",
        Ok(f64::from(percent)),
    );
}
