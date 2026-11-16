use std::{io, io::Cursor};

use tiny_http::{Header, Request, Response};

pub(crate) fn respond(request: Request) -> io::Result<()> {
    let asset = request.url().strip_prefix("/panel/").unwrap_or_default();
    // Assets are embedded from the product UI. The shortcut key is injected
    // by the panel window and is never served over HTTP.
    let (body, mime): (&[u8], _) = match asset {
        "" | "index.html" => (
            include_bytes!("../ui/panel/index.html"),
            "text/html; charset=utf-8",
        ),
        "style.css" => (
            include_bytes!("../ui/panel/style.css"),
            "text/css; charset=utf-8",
        ),
        "app.js" => (
            include_bytes!("../ui/panel/app.js"),
            "text/javascript; charset=utf-8",
        ),
        "shortcut-dock.js" => (
            include_bytes!("../ui/panel/shortcut-dock.js"),
            "text/javascript; charset=utf-8",
        ),
        "rust-sensors.js" => (
            include_bytes!("../ui/panel/rust-sensors.js"),
            "text/javascript; charset=utf-8",
        ),
        "icons/bolt.svg" => (
            include_bytes!("../ui/panel/icons/bolt.svg"),
            "image/svg+xml",
        ),
        "icons/cloud.svg" => (
            include_bytes!("../ui/panel/icons/cloud.svg"),
            "image/svg+xml",
        ),
        "icons/fog.svg" => (include_bytes!("../ui/panel/icons/fog.svg"), "image/svg+xml"),
        "icons/partly.svg" => (
            include_bytes!("../ui/panel/icons/partly.svg"),
            "image/svg+xml",
        ),
        "icons/rain.svg" => (
            include_bytes!("../ui/panel/icons/rain.svg"),
            "image/svg+xml",
        ),
        "icons/snow.svg" => (
            include_bytes!("../ui/panel/icons/snow.svg"),
            "image/svg+xml",
        ),
        "icons/storm.svg" => (
            include_bytes!("../ui/panel/icons/storm.svg"),
            "image/svg+xml",
        ),
        "icons/sun.svg" => (include_bytes!("../ui/panel/icons/sun.svg"), "image/svg+xml"),
        _ => return request.respond(Response::empty(404)),
    };
    let mut response = Response::new(
        200.into(),
        Vec::new(),
        Cursor::new(body),
        Some(body.len()),
        None,
    );
    for (name, value) in [
        ("Content-Type", mime),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
    ] {
        response.add_header(Header::from_bytes(name, value).expect("fixed panel header"));
    }
    request.respond(response)
}
