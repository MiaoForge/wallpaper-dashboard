use std::{fs, io, os::windows::process::CommandExt, process::Command, sync::Mutex, thread};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::{Method, Request, Response};

use crate::{
    service::header,
    shortcut_http::{SAME_ORIGIN, json_response, read_body},
    shortcuts::{state_file, write_atomic},
};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WeatherSettings {
    pub(crate) city: String,
}

pub(crate) fn weather() -> io::Result<WeatherSettings> {
    match fs::read(state_file("weather.json")?) {
        Ok(body) => Ok(serde_json::from_slice(&body)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(WeatherSettings::default()),
        Err(error) => Err(error),
    }
}

pub(crate) fn handle_weather(mut request: Request) -> io::Result<()> {
    let origin = header(&request, "Origin");
    let same_origin = origin.as_deref() == Some(SAME_ORIGIN);
    let result = match *request.method() {
        Method::Get if origin.is_none() || same_origin => weather(),
        Method::Post
            if same_origin
                && header(&request, "Content-Type").as_deref() == Some("application/json") =>
        {
            let Some(mut settings) = read_body::<WeatherSettings>(&mut request, 1024) else {
                return json_response(request, 400, json!({"error":"天气设置格式无效"}), false);
            };
            settings.city = settings.city.trim().to_owned();
            if settings.city.chars().count() > 100 || settings.city.chars().any(char::is_control) {
                return json_response(
                    request,
                    400,
                    json!({"error":"请输入 100 个字符以内的城市名称"}),
                    false,
                );
            }
            write_atomic(
                &state_file("weather.json")?,
                &serde_json::to_vec(&settings)?,
            )
            .map(|()| settings)
        }
        _ => return request.respond(Response::empty(403)),
    };
    match result {
        Ok(settings) => json_response(request, 200, json!(settings), false),
        Err(error) => json_response(request, 500, json!({"error":error.to_string()}), false),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupSettings {
    enabled: bool,
}

pub(crate) fn handle_startup(mut request: Request) -> io::Result<()> {
    let origin = header(&request, "Origin");
    let action = match *request.method() {
        Method::Get if origin.is_none() || origin.as_deref() == Some(SAME_ORIGIN) => "query",
        Method::Post
            if origin.as_deref() == Some(SAME_ORIGIN)
                && header(&request, "Content-Type").as_deref() == Some("application/json") =>
        {
            let Some(input) = read_body::<StartupSettings>(&mut request, 64) else {
                return json_response(request, 400, json!({"error":"无效的自启设置"}), false);
            };
            if input.enabled { "enable" } else { "disable" }
        }
        _ => return request.respond(Response::empty(403)),
    };
    thread::spawn(move || {
        static TASK: Mutex<()> = Mutex::new(());
        let Ok(_lock) = TASK.try_lock() else {
            let _ = json_response(
                request,
                409,
                json!({"error":"正在读取或保存自启设置，请重试"}),
                false,
            );
            return;
        };
        let result = (|| -> io::Result<bool> {
            let exe = std::env::current_exe()?;
            if action == "enable"
                && !exe
                    .file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("wallpaper-dashboard.exe"))
            {
                return Err(io::Error::other("use the tray collector to set startup"));
            }
            let output = Command::new(crate::shortcuts::powershell()?)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    include_str!("collector-startup.ps1"),
                ])
                .env("WALLPAPER_DASHBOARD_STARTUP", action)
                .env("WALLPAPER_DASHBOARD_EXE", exe)
                .creation_flags(0x0800_0000)
                .output()?;
            match (
                output.status.success(),
                String::from_utf8_lossy(&output.stdout).trim(),
            ) {
                (true, "enabled") => Ok(true),
                (true, "disabled") => Ok(false),
                _ => Err(io::Error::other(
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                )),
            }
        })();
        let _ = match result {
            Ok(enabled) => json_response(request, 200, json!({"enabled": enabled}), false),
            Err(error) => {
                crate::collector_log::record(&format!("collector startup {action}: {error}"));
                json_response(
                    request,
                    500,
                    json!({"error":"无法读取或保存自启设置，请以管理员身份运行托盘采集器后重试"}),
                    false,
                )
            }
        };
    });
    Ok(())
}
