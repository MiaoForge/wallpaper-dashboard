use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::json;
use tiny_http::{Method, Request, Response};

use crate::{
    service::header,
    shortcut_http::{SAME_ORIGIN, json_response},
};

const RELEASE_API: &str = "https://api.github.com/repos/MiaoForge/wallpaper-dashboard/releases";
const RELEASE_PAGE: &str = "https://github.com/MiaoForge/wallpaper-dashboard/releases/tag/";
const RELEASE_TAG: Option<&str> = option_env!("WALLPAPER_DASHBOARD_RELEASE_TAG");
static CHECKING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Development,
    Rc(u64),
    Stable,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    core: [u64; 3],
    stage: Stage,
}

fn number(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn parse_tag(tag: &str) -> Option<Version> {
    let version = tag.strip_prefix('v')?;
    let (version, stage) = match version.split_once("-rc") {
        Some((version, rc)) => (version, Stage::Rc(number(rc)?)),
        None => (version, Stage::Stable),
    };
    let mut parts = version.split('.');
    let core = [
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    ];
    parts.next().is_none().then_some(Version { core, stage })
}

fn current() -> (String, Version) {
    if let Some(tag) = RELEASE_TAG
        && let Some(version) = parse_tag(tag)
    {
        return (tag.into(), version);
    }
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let mut version = parse_tag(&tag).expect("Cargo package version is a numeric version");
    version.stage = Stage::Development;
    (tag, version)
}

pub fn is_release_page(url: &str) -> bool {
    url.strip_prefix(RELEASE_PAGE).and_then(parse_tag).is_some()
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn eligible(release: &Release, current: Version) -> Option<Version> {
    let version = parse_tag(&release.tag_name)?;
    let rc = matches!(version.stage, Stage::Rc(_));
    (!release.draft && rc == release.prerelease && (!rc || matches!(current.stage, Stage::Rc(_))))
        .then_some(version)
}

fn check(current: Version) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let timeout = Duration::from_secs(8);
    let deadline = Instant::now() + timeout;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(timeout))
        .build()
        .into();
    let mut latest: Option<(Version, String)> = None;
    for page in 1u32.. {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "release check timed out").into());
        }
        let mut response = agent
            .get(RELEASE_API)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "WallpaperDashboard")
            .query("per_page", "100")
            .query("page", page.to_string())
            .config()
            .timeout_global(Some(remaining))
            .build()
            .call()?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(8 * 1024 * 1024)
            .read_to_vec()?;
        let releases: Vec<Release> = serde_json::from_slice(&bytes)?;
        let has_next = releases.len() == 100;
        for release in releases {
            if let Some(version) = eligible(&release, current)
                && latest.as_ref().is_none_or(|(latest, _)| version > *latest)
            {
                latest = Some((version, release.tag_name));
            }
        }
        if !has_next {
            break;
        }
    }
    Ok(match latest {
        Some((version, tag)) if version > current => {
            json!({ "state": "available", "version": tag, "url": format!("{RELEASE_PAGE}{tag}") })
        }
        Some(_) => json!({ "state": "up_to_date" }),
        None => json!({ "state": "no_release" }),
    })
}

pub(crate) fn handle(request: Request) -> io::Result<()> {
    let origin = header(&request, "Origin");
    if !matches!(origin.as_deref(), None | Some(SAME_ORIGIN)) {
        return request.respond(Response::empty(403));
    }
    let (tag, version) = current();
    match *request.method() {
        Method::Get => json_response(
            request,
            200,
            json!({ "version": tag, "development": version.stage == Stage::Development }),
            false,
        ),
        Method::Post
            if origin.as_deref() == Some(SAME_ORIGIN)
                && request.body_length().unwrap_or(0) == 0 =>
        {
            if CHECKING.swap(true, Ordering::AcqRel) {
                return json_response(
                    request,
                    429,
                    json!({"error":"正在检查更新，请稍后重试"}),
                    false,
                );
            }
            let result = thread::Builder::new()
                .name("update-check".into())
                .spawn(move || {
                    let result = check(version);
                    CHECKING.store(false, Ordering::Release);
                    let (status, payload) = match result {
                        Ok(payload) => (200, payload),
                        Err(_) => (502, json!({"error":"暂时无法检查更新，请重试"})),
                    };
                    let _ = json_response(request, status, payload, false);
                });
            if result.is_err() {
                CHECKING.store(false, Ordering::Release);
            }
            result.map(|_| ())
        }
        _ => request.respond(Response::empty(403)),
    }
}

#[cfg(test)]
mod tests {
    use super::{RELEASE_PAGE, Release, eligible, is_release_page, parse_tag};

    #[test]
    fn orders_release_candidates_numerically_before_the_final_release() {
        let version = |tag| parse_tag(tag).unwrap();
        assert!(version("v0.1.0-rc2") < version("v0.1.0-rc10"));
        assert!(version("v0.1.0-rc10") < version("v0.1.0"));
        assert!(version("v0.9.0") < version("v0.10.0"));
    }

    #[test]
    fn stable_builds_skip_candidates_and_drafts() {
        let mut release = Release {
            tag_name: "v0.2.0-rc1".into(),
            draft: false,
            prerelease: true,
        };
        let stable = parse_tag("v0.1.0").unwrap();
        let rc = parse_tag("v0.1.0-rc1").unwrap();
        assert!(eligible(&release, stable).is_none());
        assert!(eligible(&release, rc).is_some());
        release.draft = true;
        assert!(eligible(&release, rc).is_none());
        release.draft = false;
        release.prerelease = false;
        release.tag_name = "v0.1.0".into();
        assert!(eligible(&release, rc).unwrap() > rc);
    }

    #[test]
    fn only_accepts_release_links_for_this_project() {
        assert!(is_release_page(&format!("{RELEASE_PAGE}v1.2.3-rc10")));
        assert!(!is_release_page(&format!(
            "{RELEASE_PAGE}v1.2.3?redirect=1"
        )));
        assert!(!is_release_page("https://example.org/releases/tag/v1.2.3"));
    }
}
