use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tiny_http::{Header, Method, Request, Response, Server};

use crate::{
    amd, battery, collector_log, frames, hid_battery, msi, nvidia,
    telemetry::{Backend, Device, Issue, unix_ms},
    windows,
};

type Sample = fn() -> Result<Vec<Device>, Issue>;
type Snapshot = Arc<RwLock<Vec<Backend>>>;

const ADDRESS: &str = "127.0.0.1:8766";
const WORKERS: [(&str, u64, Sample); 9] = [
    ("windows", 2, windows::sample),
    ("amd_adlx", 2, amd::gpu),
    ("amd_ryzen_sdk", 4, amd::cpu),
    ("msi_center", 1, msi::sample),
    ("nvidia_nvml", 2, nvidia::sample),
    ("presentmon", 1, frames::latest),
    ("windows_battery", 5, battery::sample),
    ("bluetooth_devices", 15, battery::sample_bluetooth),
    ("hid_battery", 30, hid_battery::sample),
];

pub struct Collector(Server);

impl Collector {
    pub fn bind() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Server::http(ADDRESS).map(Self)
    }

    pub fn serve(self, stop: &AtomicBool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let samples: Snapshot = Arc::new(RwLock::new(
            WORKERS
                .iter()
                .map(|(source, _, _)| {
                    Backend::new(source, Err(Issue::unavailable("waiting for first sample")))
                })
                .collect(),
        ));

        // Scoped workers are reaped on exit, including any in-progress SDK call.
        thread::scope(|scope| {
            let mut workers = Vec::new();
            let result = (|| {
                workers.push(
                    thread::Builder::new()
                        .name("recent-apps".into())
                        .spawn_scoped(scope, || crate::recent_apps::watch(stop))?,
                );
                let (weather_tx, weather_rx) = std::sync::mpsc::sync_channel(0);
                workers.push(
                    thread::Builder::new()
                        .name("weather-location".into())
                        .spawn_scoped(scope, || crate::weather_location::serve(weather_rx))?,
                );
                workers.push(
                    thread::Builder::new()
                        .name("foreground-fps".into())
                        .spawn_scoped(scope, || frames::integrated(stop))?,
                );
                for (index, (source, seconds, sample)) in WORKERS.into_iter().enumerate() {
                    let samples = Arc::clone(&samples);
                    workers.push(thread::Builder::new().name(source.into()).spawn_scoped(
                        scope,
                        move || {
                            let interval = Duration::from_secs(seconds);
                            let retention_ms = if matches!(
                                source,
                                "presentmon" | "bluetooth_devices" | "windows_battery"
                            ) {
                                0
                            } else if source == "hid_battery" {
                                45000
                            } else if source == "amd_ryzen_sdk" {
                                12000
                            } else {
                                7000
                            };
                            let mut previous_error = None;
                            while !stop.load(Ordering::Acquire) {
                                let started = Instant::now();
                                let result = sample();
                                let elapsed = started.elapsed();
                                let duration_ms = elapsed.as_millis() as u64;
                                let attempted_at = unix_ms();
                                match &result {
                                    Err(issue) => {
                                        let key = (issue.status, issue.detail.clone());
                                        if previous_error.as_ref() != Some(&key) {
                                            collector_log::record(&format!(
                                                "{source}: {:?} after {duration_ms} ms: {}",
                                                issue.status, issue.detail
                                            ));
                                            previous_error = Some(key);
                                        }
                                    }
                                    Ok(_) if previous_error.take().is_some() => {
                                        collector_log::record(&format!(
                                            "{source}: recovered in {duration_ms} ms"
                                        ))
                                    }
                                    Ok(_) => (),
                                }
                                samples.write().expect("snapshot lock poisoned")[index].record(
                                    result,
                                    attempted_at,
                                    duration_ms,
                                    retention_ms,
                                );
                                // A slow sample does not trigger catch-up polling.
                                if !stop.load(Ordering::Acquire) {
                                    thread::park_timeout(
                                        interval
                                            .saturating_sub(elapsed)
                                            .max(Duration::from_millis(250)),
                                    );
                                }
                            }
                        },
                    )?);
                }
                collector_log::record(&format!("listening on {ADDRESS}"));
                eprintln!("Wallpaper Dashboard listening on http://{ADDRESS}/");
                while !stop.load(Ordering::Acquire) {
                    if let Some(request) = self.0.recv_timeout(Duration::from_millis(250))?
                        && let Err(error) = respond(request, &samples, &weather_tx)
                    {
                        eprintln!("HTTP response: {error}");
                    }
                }
                Ok(())
            })();
            stop.store(true, Ordering::Release);
            for worker in &workers {
                worker.thread().unpark();
            }
            for worker in workers {
                if worker.join().is_err() {
                    collector_log::record("sampling worker panicked");
                }
            }
            collector_log::record("collector stopped");
            result
        })
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Collector::bind()?.serve(&AtomicBool::new(false))
}

pub(crate) fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.to_string())
}

fn respond(
    request: Request,
    samples: &Snapshot,
    weather: &std::sync::mpsc::SyncSender<Request>,
) -> std::io::Result<()> {
    // Explicit loopback Host prevents browser requests via DNS rebinding.
    if header(&request, "Host").as_deref() != Some(ADDRESS) {
        return request.respond(Response::empty(403));
    }
    if request.method() == &Method::Get && request.url().starts_with("/panel/") {
        return crate::panel_assets::respond(request);
    }
    if request.method() == &Method::Get
        && let Some(appid) = request.url().strip_prefix("/shortcuts/game-art/")
    {
        let appid = appid.to_owned();
        return crate::shortcut_http::game_art(request, appid);
    }
    if request.url() == "/settings/weather" {
        return crate::settings::handle_weather(request);
    }
    if request.url() == "/settings/startup" {
        return crate::settings::handle_startup(request);
    }
    if matches!(
        request.url(),
        "/weather-location" | "/settings/weather/test"
    ) && request.method() == &Method::Post
    {
        if header(&request, "Origin").as_deref() != Some(crate::shortcut_http::SAME_ORIGIN) {
            return request.respond(Response::empty(403));
        }
        if request.body_length().unwrap_or(0) != 0 {
            return request.respond(Response::empty(400));
        }
        return match weather.try_send(request) {
            Ok(()) => Ok(()),
            Err(
                std::sync::mpsc::TrySendError::Full(request)
                | std::sync::mpsc::TrySendError::Disconnected(request),
            ) => request.respond(Response::empty(503)),
        };
    }
    if matches!(
        request.url(),
        "/settings"
            | "/shortcuts"
            | "/shortcuts/catalog"
            | "/shortcuts/dock"
            | "/shortcuts/recent-apps"
    ) || request.url().starts_with("/shortcuts/icon/")
    {
        return crate::shortcut_http::handle(request);
    }
    if request.method() != &Method::Get {
        return request.respond(Response::empty(405));
    }

    let (body, mime, file_origin) = match request.url() {
        "/snapshots" => {
            let origin = header(&request, "Origin");
            if !matches!(
                origin.as_deref(),
                None | Some("null") | Some("http://127.0.0.1:8766")
            ) {
                return request.respond(Response::empty(403));
            }
            let weather_city = crate::settings::weather()
                .ok()
                .map(|settings| settings.city);
            let body = {
                let samples = samples.read().expect("snapshot lock poisoned");
                serde_json::to_vec(&serde_json::json!({
                    "schema_version": 1,
                    "backends": &*samples,
                        "weather_city": weather_city,
                }))?
            };
            (body, "application/json", origin.as_deref() == Some("null"))
        }
        "/" => (
            include_bytes!("../ui/overview/index.html").to_vec(),
            "text/html; charset=utf-8",
            false,
        ),
        "/dashboard.css" => (
            include_bytes!("../ui/overview/dashboard.css").to_vec(),
            "text/css; charset=utf-8",
            false,
        ),
        "/dashboard.js" => (
            include_bytes!("../ui/overview/dashboard.js").to_vec(),
            "text/javascript; charset=utf-8",
            false,
        ),
        _ => return request.respond(Response::empty(404)),
    };
    let make_header =
        |name: &str, value: &str| Header::from_bytes(name, value).expect("fixed HTTP header");
    let mut response = Response::from_data(body)
        .with_header(make_header("Content-Type", mime))
        .with_header(make_header("Cache-Control", "no-store"))
        .with_header(make_header("X-Content-Type-Options", "nosniff"));
    if file_origin {
        response.add_header(make_header("Access-Control-Allow-Origin", "null"));
        response.add_header(make_header("Vary", "Origin"));
    }
    request.respond(response)
}
