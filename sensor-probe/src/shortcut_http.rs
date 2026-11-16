use std::{
    io::{self, Read},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response};

use crate::{
    service::header,
    shortcuts::{self, Layout, Shortcut},
};

pub(crate) const SAME_ORIGIN: &str = "http://127.0.0.1:8766";
const MAX_LAYOUT: usize = 18 * (shortcuts::MAX_ICON + 512);
type Catalog = (Instant, Arc<Vec<Shortcut>>);
static CATALOG: OnceLock<Mutex<Catalog>> = OnceLock::new();
static LAUNCH: Mutex<()> = Mutex::new(());
static ICON: Mutex<()> = Mutex::new(());
static RECENT_ICON_BUSY: AtomicBool = AtomicBool::new(false);
static RECENT_ICONS: Mutex<std::collections::VecDeque<(String, Option<String>)>> =
    Mutex::new(std::collections::VecDeque::new());

pub(crate) fn cached_icon(shortcut: &Shortcut) -> Option<String> {
    if let Some((_, icon)) = RECENT_ICONS
        .lock()
        .expect("recent icons lock poisoned")
        .iter()
        .find(|(id, _)| *id == shortcut.id)
    {
        return icon.clone();
    }
    if !RECENT_ICON_BUSY.swap(true, Ordering::AcqRel) {
        let shortcut = shortcut.clone();
        thread::spawn(move || {
            let icon = shortcuts::icon(&shortcut)
                .map(|(mime, bytes)| format!("data:{mime};base64,{}", STANDARD.encode(bytes)));
            let mut icons = RECENT_ICONS.lock().expect("recent icons lock poisoned");
            icons.push_back((shortcut.id, icon));
            if icons.len() > 64 {
                icons.pop_front();
            }
            RECENT_ICON_BUSY.store(false, Ordering::Release);
        });
    }
    None
}

pub(crate) fn game_art(request: Request, appid: String) -> io::Result<()> {
    let id = format!("steam:{appid}");
    if !catalog()
        .iter()
        .any(|entry| entry.kind == "game" && entry.id == id)
    {
        return request.respond(Response::empty(404));
    }
    let Some(bytes) = shortcuts::game_art(&appid) else {
        return request.respond(Response::empty(404));
    };
    request.respond(
        Response::from_data(bytes)
            .with_header(Header::from_bytes("Content-Type", "image/jpeg").expect("fixed header"))
            .with_header(
                Header::from_bytes("Cache-Control", "private, max-age=300").expect("fixed header"),
            )
            .with_header(
                Header::from_bytes("X-Content-Type-Options", "nosniff").expect("fixed header"),
            ),
    )
}

fn catalog() -> Arc<Vec<Shortcut>> {
    let cache =
        CATALOG.get_or_init(|| Mutex::new((Instant::now(), Arc::new(shortcuts::catalog()))));
    let mut cache = cache.lock().expect("shortcut catalog lock poisoned");
    if cache.0.elapsed() >= Duration::from_secs(12) {
        *cache = (Instant::now(), Arc::new(shortcuts::catalog()));
    }
    Arc::clone(&cache.1)
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum DockRequest {
    List { key: String },
    Launch { key: String, id: String },
}

fn send(
    request: Request,
    status: u16,
    mime: &str,
    body: Vec<u8>,
    wallpaper: bool,
) -> io::Result<()> {
    let header = |name: &str, value: &str| Header::from_bytes(name, value).expect("fixed header");
    let mut response = Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", mime))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("X-Frame-Options", "DENY"));
    if wallpaper {
        response.add_header(header("Access-Control-Allow-Origin", "null"));
        response.add_header(header("Vary", "Origin"));
    }
    request.respond(response)
}

pub(crate) fn json_response(
    request: Request,
    status: u16,
    body: serde_json::Value,
    wallpaper: bool,
) -> io::Result<()> {
    send(
        request,
        status,
        "application/json",
        serde_json::to_vec(&body)?,
        wallpaper,
    )
}

pub(crate) fn read_body<T: serde::de::DeserializeOwned>(
    request: &mut Request,
    limit: usize,
) -> Option<T> {
    let length = request.body_length().filter(|&length| length <= limit)?;
    let mut body = Vec::with_capacity(length);
    request
        .as_reader()
        .take(length as u64)
        .read_to_end(&mut body)
        .ok()?;
    serde_json::from_slice(&body).ok()
}

fn layout_payload() -> io::Result<serde_json::Value> {
    let catalog = catalog();
    let layout = shortcuts::ranked_layout(shortcuts::read_layout()?, &catalog);
    let recent = crate::recent_apps::ids();
    Ok(json!({ "catalog": &*catalog, "layout": layout, "recent_apps": recent }))
}

pub fn handle(mut request: Request) -> io::Result<()> {
    let origin = header(&request, "Origin");
    if request.url() != "/shortcuts/dock"
        && header(&request, "Sec-Fetch-Site")
            .is_some_and(|site| !matches!(site.as_str(), "same-origin" | "none"))
    {
        return request.respond(Response::empty(403));
    }
    if request.method() == &Method::Get
        && (origin.is_none() || origin.as_deref() == Some(SAME_ORIGIN))
        && let Some(id) = request.url().strip_prefix("/shortcuts/icon/")
    {
        let entry = catalog().iter().find(|entry| entry.id == id).cloned();
        if let Some(entry) = entry {
            thread::spawn(move || {
                let Ok(_icon) = ICON.try_lock() else {
                    let _ = request.respond(Response::empty(429));
                    return;
                };
                let result = shortcuts::icon(&entry).map_or_else(
                    || (404, "text/plain", Vec::new()),
                    |(mime, bytes)| (200, mime, bytes),
                );
                let _ = send(request, result.0, result.1, result.2, false);
            });
            return Ok(());
        }
        return request.respond(Response::empty(404));
    }
    // The file-origin wallpaper must prove possession of its per-user pairing key.
    // The settings page accepts writes only from its exact browser origin.
    match (request.method(), request.url()) {
        (&Method::Get, "/shortcuts/recent-apps")
            if origin.is_none() || origin.as_deref() == Some(SAME_ORIGIN) =>
        {
            json_response(request, 200, json!(crate::recent_apps::ids()), false)
        }
        (&Method::Get, "/settings" | "/shortcuts")
            if origin.is_none() || origin.as_deref() == Some(SAME_ORIGIN) =>
        {
            send(
                request,
                200,
                "text/html; charset=utf-8",
                include_bytes!("../ui/shortcut-settings.html").to_vec(),
                false,
            )
        }
        (&Method::Get, "/shortcuts/catalog")
            if origin.is_none() || origin.as_deref() == Some(SAME_ORIGIN) =>
        {
            match layout_payload() {
                Ok(body) => json_response(request, 200, body, false),
                Err(error) => {
                    json_response(request, 500, json!({"error":error.to_string()}), false)
                }
            }
        }
        (&Method::Post, "/shortcuts/catalog")
            if origin.as_deref() == Some(SAME_ORIGIN)
                && header(&request, "Content-Type").as_deref() == Some("application/json") =>
        {
            let Some(layout) = read_body::<Layout>(&mut request, MAX_LAYOUT) else {
                return json_response(request, 400, json!({"error":"配置格式无效"}), false);
            };
            match shortcuts::save_layout(&layout, &catalog()) {
                Ok(()) => json_response(request, 200, json!({"ok":true}), false),
                Err(error) => {
                    json_response(request, 400, json!({"error":error.to_string()}), false)
                }
            }
        }
        (&Method::Post, "/shortcuts/dock")
            if matches!(origin.as_deref(), Some("null" | SAME_ORIGIN))
                && header(&request, "Content-Type")
                    .is_some_and(|value| value.split(';').next() == Some("text/plain")) =>
        {
            let Some(input) = read_body::<DockRequest>(&mut request, 512) else {
                return json_response(request, 400, json!({"error":"请求无效"}), true);
            };
            let key = match &input {
                DockRequest::List { key } | DockRequest::Launch { key, .. } => key,
            };
            if !shortcuts::authorized(key) {
                return json_response(request, 403, json!({"error":"壁纸未配对"}), true);
            }
            let catalog = catalog();
            let layout = match shortcuts::read_layout() {
                Ok(layout) => shortcuts::ranked_layout(layout, &catalog),
                Err(error) => {
                    return json_response(request, 500, json!({"error":error.to_string()}), true);
                }
            };
            match input {
                DockRequest::List { .. } => json_response(
                    request,
                    200,
                    json!(shortcuts::dock(&layout, &catalog)),
                    true,
                ),
                DockRequest::Launch { id, .. } => {
                    let Some(shortcut) = shortcuts::bound_shortcut(&id, &layout, &catalog).cloned()
                    else {
                        return json_response(
                            request,
                            404,
                            json!({"error":"快捷方式未绑定或已移除"}),
                            true,
                        );
                    };
                    thread::spawn(move || {
                        let Ok(_launch) = LAUNCH.try_lock() else {
                            let _ = json_response(
                                request,
                                429,
                                json!({"error":"另一个快捷方式正在打开"}),
                                true,
                            );
                            return;
                        };
                        let (status, body) = match shortcuts::launch(&shortcut) {
                            Ok(()) => (200, json!({"ok":true})),
                            Err(error) => (500, json!({"error":error.to_string()})),
                        };
                        let _ = json_response(request, status, body, true);
                    });
                    Ok(())
                }
            }
        }
        _ => request.respond(Response::empty(403)),
    }
}
