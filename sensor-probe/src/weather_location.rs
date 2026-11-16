use std::{sync::mpsc::Receiver, time::Duration};

use serde::{Deserialize, Serialize};
use tiny_http::{Header, Request, Response};

#[derive(Deserialize, Serialize)]
struct Location {
    city: String,
    lat: f64,
    lon: f64,
    tz_id: String,
}

#[derive(Deserialize)]
struct Lookup {
    success: bool,
    city: String,
    latitude: f64,
    longitude: f64,
    timezone: Timezone,
}

#[derive(Deserialize)]
struct Timezone {
    id: String,
}

fn locate(agent: &ureq::Agent) -> Option<Location> {
    let mut response = agent
        .get("https://ipwho.is/")
        .query("fields", "success,city,latitude,longitude,timezone")
        .call()
        .ok()?;
    let body = response
        .body_mut()
        .with_config()
        .limit(8192)
        .read_to_vec()
        .ok()?;
    let lookup: Lookup = serde_json::from_slice(&body).ok()?;
    (lookup.success
        && !lookup.city.trim().is_empty()
        && !lookup.timezone.id.trim().is_empty()
        && (-90.0..=90.0).contains(&lookup.latitude)
        && (-180.0..=180.0).contains(&lookup.longitude))
    .then_some(Location {
        city: lookup.city,
        lat: lookup.latitude,
        lon: lookup.longitude,
        tz_id: lookup.timezone.id,
    })
}

pub(crate) fn serve(queue: Receiver<Request>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(6)))
        .build()
        .into();
    for request in queue {
        let result = locate(&agent)
            .and_then(|location| serde_json::to_vec(&location).ok())
            .ok_or(502);
        let (status, body) = match result {
            Ok(body) => (200, body),
            Err(status) => (status, Vec::new()),
        };
        let mut response = Response::from_data(body).with_status_code(status);
        for (name, value) in [
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-store"),
            ("X-Content-Type-Options", "nosniff"),
        ] {
            response.add_header(Header::from_bytes(name, value).expect("fixed HTTP header"));
        }
        let _ = request.respond(response);
    }
}
