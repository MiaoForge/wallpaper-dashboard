use std::{
    collections::{BTreeSet, HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    os::windows::{ffi::OsStringExt, fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use keyvalues_parser::{Obj, Value, Vdf};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows_sys::Win32::{
    System::Com::CoTaskMemFree,
    UI::Shell::{
        FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_PublicDesktop,
        SHGetKnownFolderPath,
    },
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

#[derive(Clone, Serialize)]
pub struct Shortcut {
    pub id: String,
    pub name: String,
    pub source: &'static str,
    pub kind: &'static str,
    #[serde(skip)]
    pub(crate) last_played: u64,
    #[serde(skip)]
    target: OsString,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub apps: Vec<Binding>,
    pub games: Vec<Binding>,
}

#[derive(Serialize)]
pub struct Tile {
    #[serde(flatten)]
    shortcut: Shortcut,
    icon: Option<String>,
    recent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    artwork: Option<String>,
}

impl Tile {
    fn new(shortcut: &Shortcut, icon: Option<String>, recent: bool) -> Self {
        let artwork = (shortcut.kind == "game" && (icon.is_none() || recent))
            .then(|| shortcut.id.strip_prefix("steam:"))
            .flatten()
            .map(|id| format!("/shortcuts/game-art/{id}"));
        Self {
            shortcut: shortcut.clone(),
            icon,
            recent,
            artwork,
        }
    }
}

#[derive(Serialize)]
pub struct Dock {
    apps: Vec<Tile>,
    games: Vec<Tile>,
}

const APP_LIMIT: usize = 11;
const GAME_LIMIT: usize = 7;

pub(crate) fn state_file(name: &str) -> io::Result<PathBuf> {
    let root =
        env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA unavailable"))?;
    Ok(PathBuf::from(root).join("WallpaperDashboard").join(name))
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let folder = path
        .parent()
        .ok_or_else(|| io::Error::other("invalid state path"))?;
    fs::create_dir_all(folder)?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn token_valid(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn pair(folder: &Path) -> io::Result<()> {
    let folder = folder.canonicalize()?;
    if !folder.join("shortcut-dock.js").is_file() || !folder.join("index.html").is_file() {
        return Err(io::Error::other(
            "select the Rust reference wallpaper folder",
        ));
    }
    let token = dock_pairing_key()?;
    let script = format!("window.wallpaperShortcutKey = \"{token}\";\n");
    write_atomic(&folder.join("shortcut-pairing.js"), script.as_bytes())
}

pub fn dock_pairing_key() -> io::Result<String> {
    let token_path = state_file("shortcut-pairing.key")?;
    let token = match fs::read_to_string(&token_path) {
        Ok(token) if token_valid(&token) => token,
        Ok(_) => return Err(io::Error::other("invalid existing pairing key")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut random = [0u8; 32];
            getrandom::fill(&mut random).map_err(io::Error::other)?;
            let token = hex::encode(random);
            // Preserve an existing key if another pairing runs concurrently.
            let folder = token_path.parent().expect("state file has a parent");
            fs::create_dir_all(folder)?;
            let mut file = tempfile::NamedTempFile::new_in(folder)?;
            file.write_all(token.as_bytes())?;
            match file.persist_noclobber(&token_path) {
                Ok(_) => token,
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                    fs::read_to_string(&token_path)?
                }
                Err(error) => return Err(error.error),
            }
        }
        Err(error) => return Err(error),
    };
    if !token_valid(&token) {
        return Err(io::Error::other("invalid pairing key"));
    }
    Ok(token)
}

pub fn authorized(token: &str) -> bool {
    if !token_valid(token) {
        return false;
    }
    let Ok(stored) = state_file("shortcut-pairing.key").and_then(fs::read_to_string) else {
        return false;
    };
    token_valid(&stored)
        && stored
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

pub fn read_layout() -> io::Result<Layout> {
    match fs::read(state_file("shortcuts.json")?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Layout::default()),
        Err(error) => Err(error),
    }
}

pub(crate) const MAX_ICON: usize = 384 * 1024;

fn valid_icon(icon: &str) -> bool {
    icon.len() <= MAX_ICON
        && icon
            .strip_prefix("data:image/png;base64,")
            .and_then(|data| STANDARD.decode(data).ok())
            .is_some_and(|bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
}

pub fn save_layout(layout: &Layout, catalog: &[Shortcut]) -> io::Result<()> {
    let ids: HashMap<_, _> = catalog
        .iter()
        .map(|item| (item.id.as_str(), item.kind))
        .collect();
    for (row, limit, kind) in [
        (&layout.apps, APP_LIMIT, "app"),
        (&layout.games, GAME_LIMIT, "game"),
    ] {
        let mut seen = HashSet::new();
        if row.len() > limit
            || row.iter().any(|item| {
                ids.get(item.id.as_str()).copied() != Some(kind)
                    || !seen.insert(item.id.as_str())
                    || item.icon.as_deref().is_some_and(|icon| !valid_icon(icon))
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown, duplicate or invalid shortcut",
            ));
        }
    }
    write_atomic(&state_file("shortcuts.json")?, &serde_json::to_vec(layout)?)
}

pub fn dock(layout: &Layout, catalog: &[Shortcut]) -> Dock {
    let shortcuts: HashMap<_, _> = catalog.iter().map(|entry| (&entry.id, entry)).collect();
    let tiles = |row: &[Binding], limit: usize, kind: &str, recent: Vec<&Shortcut>| {
        let mut seen = HashSet::new();
        let mut tiles: Vec<_> = row
            .iter()
            .filter_map(|binding| {
                let shortcut = shortcuts.get(&binding.id)?;
                if shortcut.kind != kind || !seen.insert(shortcut.id.as_str()) {
                    return None;
                }
                Some(Tile::new(shortcut, binding.icon.clone(), false))
            })
            .take(limit)
            .collect();
        for shortcut in recent {
            if tiles.len() == limit {
                break;
            }
            if seen.insert(&shortcut.id) {
                let icon = (kind == "app")
                    .then(|| crate::shortcut_http::cached_icon(shortcut))
                    .flatten();
                tiles.push(Tile::new(shortcut, icon, true));
            }
        }
        tiles
    };
    let app_ids = crate::recent_apps::ids();
    let app_recent = app_ids
        .iter()
        .filter_map(|id| shortcuts.get(id).copied())
        .filter(|shortcut| shortcut.kind == "app")
        .collect();
    let mut game_recent: Vec<_> = catalog
        .iter()
        .filter(|shortcut| shortcut.kind == "game" && shortcut.last_played > 0)
        .collect();
    game_recent.sort_by_key(|shortcut| (std::cmp::Reverse(shortcut.last_played), &shortcut.id));
    Dock {
        apps: tiles(&layout.apps, APP_LIMIT, "app", app_recent),
        games: tiles(&layout.games, GAME_LIMIT, "game", game_recent),
    }
}

pub fn bound_shortcut<'a>(
    id: &str,
    layout: &Layout,
    catalog: &'a [Shortcut],
) -> Option<&'a Shortcut> {
    let dock = dock(layout, catalog);
    dock.apps
        .iter()
        .chain(&dock.games)
        .any(|tile| tile.shortcut.id == id)
        .then(|| catalog.iter().find(|shortcut| shortcut.id == id))
        .flatten()
}

fn folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    // SAFETY: SHGetKnownFolderPath provides a NUL-terminated, task-allocated path.
    unsafe {
        let mut path = ptr::null_mut();
        if SHGetKnownFolderPath(id, 0, ptr::null_mut(), &mut path) < 0 || path.is_null() {
            return None;
        }
        let mut length = 0;
        while *path.add(length) != 0 {
            length += 1;
        }
        let folder = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
            path, length,
        )));
        CoTaskMemFree(path.cast());
        Some(folder)
    }
}

pub(crate) fn windows_shortcuts() -> Vec<Shortcut> {
    let mut pending: Vec<_> = [
        (&FOLDERID_Programs, true),
        (&FOLDERID_CommonPrograms, true),
        (&FOLDERID_Desktop, false),
        (&FOLDERID_PublicDesktop, false),
    ]
    .into_iter()
    .filter_map(|(id, recurse)| folder(id).map(|path| (path, recurse)))
    .collect();
    let mut seen = HashSet::new();
    let mut shortcuts = Vec::new();
    while let Some((directory, recurse)) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            if meta.file_attributes() & 0x400 != 0 {
                continue;
            }
            if meta.is_dir() {
                if recurse {
                    pending.push((path, recurse));
                }
            } else if meta.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            {
                let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                let id = format!(
                    "win:{}",
                    hex::encode(Sha256::digest(path.to_string_lossy().to_lowercase()))
                );
                if seen.insert(id.clone()) {
                    shortcuts.push(Shortcut {
                        id,
                        name: name.to_owned(),
                        source: "Windows",
                        kind: "app",
                        last_played: 0,
                        target: path.clone().into_os_string(),
                    });
                }
            }
        }
    }
    shortcuts
}

fn item<'a>(object: &'a Obj<'_>, key: &str) -> Option<&'a Value<'a>> {
    object.get(key)?.first()
}

fn steam_games() -> Vec<(Shortcut, PathBuf)> {
    let Ok(reg) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam") else {
        return Vec::new();
    };
    let Ok(root) = reg.get_value::<String, _>("SteamPath") else {
        return Vec::new();
    };
    let root = PathBuf::from(root);
    let mut libraries = BTreeSet::from([root.clone()]);
    if let Ok(data) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf"))
        && let Ok(vdf) = keyvalues_parser::parse(&data).map(Vdf::from)
        && let Some(folders) = vdf.value.get_obj()
    {
        for (key, values) in folders.iter() {
            if key.parse::<u32>().is_err() {
                continue;
            }
            for value in values {
                let path = value
                    .get_str()
                    .or_else(|| item(value.get_obj()?, "path")?.get_str());
                if let Some(path) = path.map(PathBuf::from).filter(|path| path.is_absolute()) {
                    libraries.insert(path);
                }
            }
        }
    }
    let mut ids = HashSet::new();
    let mut games = Vec::new();
    for library in libraries {
        let depot = library.join("steamapps");
        let Ok(files) = fs::read_dir(&depot) else {
            continue;
        };
        for file in files.flatten() {
            let filename = file.file_name();
            let Some(appid) = filename
                .to_str()
                .and_then(|name| name.strip_prefix("appmanifest_"))
                .and_then(|name| name.strip_suffix(".acf"))
                .and_then(|id| id.parse::<u32>().ok())
                .filter(|id| *id != 0)
            else {
                continue;
            };
            let Ok(data) = fs::read_to_string(file.path()) else {
                continue;
            };
            let Ok(vdf) = keyvalues_parser::parse(&data).map(Vdf::from) else {
                continue;
            };
            let Some(app) = vdf.value.get_obj() else {
                continue;
            };
            let field = |key| item(app, key).and_then(Value::get_str);
            let Some(name) = field("name").filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            let Some(installdir) = field("installdir").filter(|dir| {
                matches!(
                    Path::new(dir).components().collect::<Vec<_>>().as_slice(),
                    [std::path::Component::Normal(_)]
                )
            }) else {
                continue;
            };
            let installed = field("StateFlags")
                .and_then(|flags| flags.parse::<u32>().ok())
                .is_some_and(|flags| flags & 4 != 0);
            let install_path = depot.join("common").join(installdir);
            if installed && install_path.is_dir() && ids.insert(appid) {
                games.push((
                    Shortcut {
                        id: format!("steam:{appid}"),
                        name: name.to_owned(),
                        source: "Steam",
                        kind: if matches!(appid, 431960 | 250820) {
                            "app"
                        } else {
                            "game"
                        },
                        last_played: field("LastPlayed")
                            .and_then(|value| value.parse().ok())
                            .unwrap_or(0),
                        target: format!("steam://rungameid/{appid}").into(),
                    },
                    install_path,
                ));
            }
        }
    }
    games
}

pub fn steam_game_directories() -> Vec<PathBuf> {
    steam_games()
        .into_iter()
        .filter(|(entry, _)| {
            !matches!(
                entry.id.as_str(),
                "steam:431960" | "steam:228980" | "steam:250820"
            )
        })
        .map(|(_, path)| path)
        .collect()
}

pub fn windows_game_executables() -> Vec<PathBuf> {
    let Ok(layout) = read_layout() else {
        return Vec::new();
    };
    if !layout
        .games
        .iter()
        .any(|binding| binding.id.starts_with("win:"))
    {
        return Vec::new();
    }
    let paths: Vec<_> = windows_shortcuts()
        .into_iter()
        .filter(|entry| layout.games.iter().any(|binding| binding.id == entry.id))
        .map(|entry| entry.target.to_string_lossy().into_owned())
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    let (Ok(exe), Ok(paths)) = (powershell(), serde_json::to_string(&paths)) else {
        return Vec::new();
    };
    let Ok(output) = Command::new(exe)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("shortcut-game-targets.ps1"),
        ])
        .env("WALLPAPER_DASHBOARD_GAME_LINKS", paths)
        .creation_flags(0x0800_0000)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}
pub fn catalog() -> Vec<Shortcut> {
    let mut catalog = windows_shortcuts();
    catalog.extend(packaged_apps());
    catalog.extend(
        steam_games()
            .into_iter()
            .map(|(entry, _)| entry)
            .filter(|entry| entry.id != "steam:228980"),
    );
    catalog.sort_by_cached_key(|item| (item.source, item.name.to_lowercase()));
    catalog
}

fn packaged_apps() -> Vec<Shortcut> {
    static CACHE: OnceLock<Mutex<(Instant, Vec<Shortcut>)>> = OnceLock::new();
    let load = || {
        let mut entries = Vec::new();
        if let Ok(exe) = powershell()
            && let Ok(output) = Command::new(exe)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    include_str!("shortcut-packaged-apps.ps1"),
                ])
                .creation_flags(0x0800_0000)
                .output()
            && output.status.success()
            && let Ok(apps) = serde_json::from_slice::<Vec<(String, String)>>(&output.stdout)
        {
            for (name, appid) in apps {
                if !name.trim().is_empty()
                    && appid.contains('!')
                    && appid
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-!".contains(&byte))
                {
                    entries.push(Shortcut {
                        id: format!("appx:{}", appid.to_ascii_lowercase()),
                        name,
                        source: "Windows",
                        kind: "app",
                        last_played: 0,
                        target: format!(r"shell:AppsFolder\{appid}").into(),
                    });
                }
            }
        }
        (Instant::now(), entries)
    };
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(load()))
        .lock()
        .expect("packaged apps cache poisoned");
    if cache.0.elapsed() >= Duration::from_secs(120) {
        *cache = load();
    }
    cache.1.clone()
}

pub(crate) fn ranked_layout(mut layout: Layout, catalog: &[Shortcut]) -> Layout {
    for kind in ["app", "game"] {
        let (destination, source) = if kind == "app" {
            (&mut layout.apps, &mut layout.games)
        } else {
            (&mut layout.games, &mut layout.apps)
        };
        let limit = if kind == "app" { APP_LIMIT } else { GAME_LIMIT };
        let mut index = 0;
        while index < source.len() {
            let id = &source[index].id;
            let belongs = catalog
                .iter()
                .any(|entry| entry.id == *id && entry.kind == kind);
            if belongs && destination.iter().any(|entry| entry.id == *id) {
                source.remove(index);
            } else if belongs && destination.len() < limit {
                destination.push(source.remove(index));
            } else {
                index += 1;
            }
        }
    }
    layout
}

fn steam_cache(appid: &str) -> Option<PathBuf> {
    let registry = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Valve\Steam")
        .ok()?;
    let root = PathBuf::from(registry.get_value::<String, _>("SteamPath").ok()?);
    Some(root.join("appcache/librarycache").join(appid))
}

fn jpeg(path: &Path, size_limit: u64) -> Option<Vec<u8>> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > size_limit {
        return None;
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, size_limit + 1), &mut bytes).ok()?;
    (bytes.len() as u64 <= size_limit && bytes.starts_with(&[0xFF, 0xD8, 0xFF])).then_some(bytes)
}

pub(crate) fn game_art(appid: &str) -> Option<Vec<u8>> {
    // Steam cache formats include both flat files and hashed subdirectories.
    if appid.parse::<u32>().is_err() || !appid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let root = steam_cache(appid)?;
    let mut folders = vec![root.clone()];
    folders.extend(
        fs::read_dir(root)
            .ok()?
            .flatten()
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && entry.file_name().to_str().is_some_and(|name| {
                        name.len() == 40 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            })
            .map(|entry| entry.path()),
    );
    for name in [
        "library_600x900.jpg",
        "library_capsule.jpg",
        "header.jpg",
        "library_header.jpg",
    ] {
        if let Some(image) = folders
            .iter()
            .find_map(|folder| jpeg(&folder.join(name), 512 * 1024))
        {
            return Some(image);
        }
    }
    None
}

pub fn icon(shortcut: &Shortcut) -> Option<(&'static str, Vec<u8>)> {
    if let Some(appid) = shortcut.id.strip_prefix("steam:") {
        let files = fs::read_dir(steam_cache(appid)?).ok()?;
        for file in files.flatten() {
            let filename = file.file_name();
            let Some(hash) = filename.to_str().and_then(|name| name.strip_suffix(".jpg")) else {
                continue;
            };
            if hash.len() == 40
                && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                && let Some(bytes) = jpeg(&file.path(), 128 * 1024)
            {
                return Some(("image/jpeg", bytes));
            }
        }
        return None;
    }
    let (script, variable, value) = if let Some(appid) = shortcut.id.strip_prefix("appx:") {
        (
            include_str!("shortcut-packaged-icon.ps1"),
            "WALLPAPER_DASHBOARD_APP_ID",
            std::ffi::OsStr::new(appid),
        )
    } else {
        (
            include_str!("shortcut-icon.ps1"),
            "WALLPAPER_DASHBOARD_ICON_LINK",
            shortcut.target.as_os_str(),
        )
    };
    let output = Command::new(powershell().ok()?)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env(variable, value)
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > MAX_ICON {
        return None;
    }
    let bytes = STANDARD.decode(output.stdout).ok()?;
    bytes
        .starts_with(b"\x89PNG\r\n\x1a\n")
        .then_some(("image/png", bytes))
}

pub(crate) fn powershell() -> io::Result<PathBuf> {
    let system =
        env::var_os("SystemRoot").ok_or_else(|| io::Error::other("SystemRoot unavailable"))?;
    Ok(PathBuf::from(system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
}

pub(crate) fn application_targets() -> HashMap<String, String> {
    let mut shortcuts = windows_shortcuts();
    shortcuts
        .sort_by_key(|shortcut| (shortcut.target.to_string_lossy().len(), shortcut.id.clone()));
    let paths: Vec<_> = shortcuts
        .iter()
        .map(|shortcut| shortcut.target.to_string_lossy())
        .collect();
    let Ok(exe) = powershell() else {
        return HashMap::new();
    };
    let Ok(input) = serde_json::to_vec(&paths) else {
        return HashMap::new();
    };
    let Ok(mut child) = Command::new(exe)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("shortcut-app-targets.ps1"),
        ])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashMap::new();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&input);
        let _ = stdin.write_all(b"\n");
    }
    let Ok(output) = child.wait_with_output() else {
        return HashMap::new();
    };
    let Ok(paths) = serde_json::from_slice::<Vec<(usize, String)>>(&output.stdout) else {
        return HashMap::new();
    };
    let mut targets = HashMap::new();
    for (index, target) in paths {
        if let Some(shortcut) = shortcuts.get(index) {
            if let Ok(path) = Path::new(&target).canonicalize() {
                targets
                    .entry(crate::frames::path_key(&path))
                    .or_insert_with(|| shortcut.id.clone());
            }
            targets
                .entry(crate::frames::path_key(Path::new(&target)))
                .or_insert_with(|| shortcut.id.clone());
        }
    }
    targets
}

pub fn launch(shortcut: &Shortcut) -> io::Result<()> {
    if shortcut.id.starts_with("win:") && !Path::new(&shortcut.target).is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Windows shortcut is missing",
        ));
    }
    // Delegate to the existing Explorer desktop. The selected path travels as data,
    // and launched apps use the desktop user's token even if the collector is elevated.
    open_in_explorer(&shortcut.target, "")
}

pub fn open_settings() -> io::Result<()> {
    open_in_explorer(env::current_exe()?.as_os_str(), "--settings-window")
}

pub fn open_panel() -> io::Result<()> {
    open_in_explorer(env::current_exe()?.as_os_str(), "--toggle-panel")
}

fn open_in_explorer(target: &std::ffi::OsStr, arguments: &str) -> io::Result<()> {
    let output = Command::new(powershell()?)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("launch-shortcut.ps1"),
        ])
        .env("WALLPAPER_DASHBOARD_SHORTCUT", target)
        .env("WALLPAPER_DASHBOARD_ARGUMENTS", arguments)
        .creation_flags(0x0800_0000)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Explorer could not open the shortcut"))
    }
}
