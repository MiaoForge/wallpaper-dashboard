use std::{
    fs, io,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    ptr,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tao::{
    dpi::PhysicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    monitor::MonitorHandle,
    platform::{
        run_return::EventLoopExtRunReturn,
        windows::{MonitorHandleExtWindows, WindowBuilderExtWindows, WindowExtWindows},
    },
    window::{Window, WindowBuilder},
};
use windows_sys::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            CombineRgn, CreateRectRgn, CreateRoundRectRgn, DISPLAY_DEVICEW, DeleteObject,
            EnumDisplayDevicesW, GetMonitorInfoW, MONITORINFO, RGN_OR, ScreenToClient,
            SetWindowRgn,
        },
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, ShellExecuteW},
            WindowsAndMessaging::{
                CS_OWNDC, EDD_GET_DEVICE_INTERFACE_NAME, FindWindowExW, GCL_STYLE, GW_HWNDNEXT,
                GWL_EXSTYLE, GetClassLongPtrW, GetClientRect, GetShellWindow, GetTopWindow,
                GetWindow, GetWindowLongPtrW, HWND_TOP, IsWindowVisible, LWA_ALPHA, PostMessageW,
                SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SetClassLongPtrW,
                SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WM_CLOSE,
                WM_NCCALCSIZE, WM_NCDESTROY, WS_EX_LAYERED,
            },
        },
    },
    w,
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

use sensor_probe::{collector_log, shortcuts};

const CLASS_NAME: &str = "WallpaperDashboardDesktopPanel";
const PANEL_URL: &str = "http://127.0.0.1:8766/panel/";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Placement {
    monitor: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn monitor_id(monitor: &MonitorHandle) -> String {
    let adapter = monitor.native_id();
    let name: Vec<_> = adapter.encode_utf16().chain([0]).collect();
    let mut device = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    // SAFETY: name is terminated and device has the required structure size.
    if unsafe { EnumDisplayDevicesW(name.as_ptr(), 0, &mut device, EDD_GET_DEVICE_INTERFACE_NAME) }
        != 0
    {
        let end = device
            .DeviceID
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(device.DeviceID.len());
        if end > 0 {
            return String::from_utf16_lossy(&device.DeviceID[..end]);
        }
    }
    adapter
}

fn work_area(monitor: &MonitorHandle) -> RECT {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: The handle comes from Tao and info has the required size.
    if unsafe { GetMonitorInfoW(monitor.hmonitor() as _, &mut info) } != 0 {
        return info.rcWork;
    }
    let origin = monitor.position();
    let size = monitor.size();
    RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + size.width as i32,
        bottom: origin.y + size.height as i32,
    }
}

fn position_window(
    window: &Window,
    parent: HWND,
    monitor: &MonitorHandle,
    saved: Option<&Placement>,
) -> io::Result<Placement> {
    let area = work_area(monitor);
    let width = (area.right - area.left).max(1);
    let height = (area.bottom - area.top).max(1);
    let default_width = width;
    let default_height = height;
    let mut placement = saved.cloned().unwrap_or_else(|| Placement {
        monitor: monitor_id(monitor),
        x: (width - default_width) / 2,
        y: (height - default_height) / 2,
        width: default_width as u32,
        height: default_height as u32,
    });
    placement.width = placement.width.clamp(420.min(width) as u32, width as u32);
    placement.height = placement
        .height
        .clamp(320.min(height) as u32, height as u32);
    let drag_margin_x = (width / 20).min(100);
    let drag_margin_y = (height / 20).min(100);
    placement.x = placement.x.clamp(
        -drag_margin_x,
        width - placement.width as i32 + drag_margin_x,
    );
    placement.y = placement.y.clamp(
        -drag_margin_y,
        height - placement.height as i32 + drag_margin_y,
    );
    let mut point = POINT {
        x: area.left + placement.x,
        y: area.top + placement.y,
    };
    // SAFETY: Both handles are live and the pointer is writable. A child window
    // uses desktop-host client coordinates rather than virtual screen coordinates.
    let positioned = unsafe {
        ScreenToClient(parent, &mut point) != 0
            && SetWindowPos(
                window.hwnd() as HWND,
                HWND_TOP,
                point.x,
                point.y,
                placement.width as i32,
                placement.height as i32,
                SWP_NOACTIVATE | SWP_FRAMECHANGED,
            ) != 0
    };
    if !positioned {
        return Err(io::Error::last_os_error());
    }
    Ok(placement)
}

fn capture_placement(window: &Window, placement: &mut Placement) {
    let Some(monitor) = window.current_monitor() else {
        return;
    };
    placement.monitor = monitor_id(&monitor);
    if let Ok(position) = window.outer_position() {
        let area = work_area(&monitor);
        let size = window.outer_size();
        placement.x = position.x - area.left;
        placement.y = position.y - area.top;
        placement.width = size.width;
        placement.height = size.height;
    }
}

fn save_placement(path: &std::path::Path, placement: &Placement) -> io::Result<()> {
    let parent = path.parent().expect("panel state directory");
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, placement)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

unsafe extern "system" fn borderless_child(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    if message == WM_NCCALCSIZE {
        // The entire desktop child rectangle is usable content.
        return 0;
    }
    // SAFETY: This subclass runs on the owning UI thread.
    unsafe {
        if message == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(borderless_child), id);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

fn desktop_host() -> io::Result<HWND> {
    // Find the desktop icon host. On current Windows 11 it is a child of
    // Progman, alongside the wallpaper layer. Older shells use a WorkerW.
    // SAFETY: These read-only calls use static class names and system HWNDs.
    unsafe {
        let shell = GetShellWindow();
        if !shell.is_null()
            && !FindWindowExW(shell, ptr::null_mut(), w!("SHELLDLL_DefView"), ptr::null()).is_null()
        {
            return Ok(shell);
        }
        let mut window = GetTopWindow(ptr::null_mut());
        while !window.is_null() {
            if IsWindowVisible(window) != 0
                && !FindWindowExW(window, ptr::null_mut(), w!("SHELLDLL_DefView"), ptr::null())
                    .is_null()
            {
                return Ok(window);
            }
            window = GetWindow(window, GW_HWNDNEXT);
        }
    }
    Err(io::Error::other("找不到 Windows 桌面层"))
}

pub(super) fn close() -> io::Result<bool> {
    let parent = desktop_host()?;
    let class: Vec<_> = CLASS_NAME.encode_utf16().chain([0]).collect();
    // SAFETY: class is terminated, and WM_CLOSE requests a normal shutdown.
    unsafe {
        let panel = FindWindowExW(parent, ptr::null_mut(), class.as_ptr(), ptr::null());
        if !panel.is_null() {
            if PostMessageW(panel, WM_CLOSE, 0, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn toggle() -> Result<(), super::Error> {
    if close()? { Ok(()) } else { run() }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum PanelMessage {
    Region {
        viewport: [f64; 2],
        cards: Vec<[f64; 5]>,
    },
    Move {
        delta: [f64; 2],
    },
    LaunchApp {
        request_id: u32,
        target: PathBuf,
    },
}

fn launch_app(hwnd: HWND, target: &std::path::Path) -> Result<(), String> {
    if !target.is_absolute()
        || !target
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        || !target.is_file()
    {
        return Err("Windows 快捷方式无效或已移除".into());
    }
    let target: Vec<_> = target.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: the shortcut path is terminated and hwnd belongs to the panel.
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            w!("open"),
            target.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        return Err(format!("Windows 无法打开快捷方式（错误 {result}）"));
    }
    Ok(())
}

fn apply_region(window: &Window, viewport: [f64; 2], cards: &[[f64; 5]]) -> io::Result<()> {
    if cards.len() > 16
        || viewport
            .iter()
            .any(|value| *value < 1.0 || *value > 32768.0)
    {
        return Ok(());
    }
    let hwnd = window.hwnd() as HWND;
    let mut bounds = RECT::default();
    // SAFETY: HWND is live on this thread and bounds is writable.
    if unsafe { GetClientRect(hwnd, &mut bounds) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if let Some(monitor) = window.current_monitor()
        && let Ok(position) = window.outer_position()
    {
        let work = work_area(&monitor);
        bounds.left = (work.left - position.x).max(0);
        bounds.top = (work.top - position.y).max(0);
        bounds.right = (work.right - position.x).min(bounds.right);
        bounds.bottom = (work.bottom - position.y).min(bounds.bottom);
    }
    let size = window.inner_size();
    let sx = f64::from(size.width) / viewport[0];
    let sy = f64::from(size.height) / viewport[1];
    // SAFETY: The regions are owned until released or passed to SetWindowRgn.
    unsafe {
        let region = CreateRectRgn(0, 0, 0, 0);
        if region.is_null() {
            return Err(io::Error::last_os_error());
        }
        for &[left, top, right, bottom, radius] in cards {
            let l = (left * sx)
                .clamp(
                    f64::from(bounds.left.min(bounds.right)),
                    f64::from(bounds.right),
                )
                .floor() as i32;
            let t = (top * sy)
                .clamp(
                    f64::from(bounds.top.min(bounds.bottom)),
                    f64::from(bounds.bottom),
                )
                .floor() as i32;
            let r = (right * sx)
                .clamp(
                    f64::from(bounds.left.min(bounds.right)),
                    f64::from(bounds.right),
                )
                .ceil() as i32;
            let b = (bottom * sy)
                .clamp(
                    f64::from(bounds.top.min(bounds.bottom)),
                    f64::from(bounds.bottom),
                )
                .ceil() as i32;
            if r > l && b > t {
                let diameter =
                    (radius * sx.min(sy) * 2.0).clamp(0.0, f64::from((r - l).min(b - t))) as i32;
                let card = CreateRoundRectRgn(l, t, r, b, diameter, diameter);
                if !card.is_null() {
                    CombineRgn(region, region, card, RGN_OR);
                    DeleteObject(card);
                }
            }
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            let error = io::Error::last_os_error();
            DeleteObject(region);
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn run() -> Result<(), super::Error> {
    let Some(_instance) = super::settings_window::claim_window(CLASS_NAME)? else {
        return Ok(());
    };
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("无法读取本地应用数据目录"))?
        .join("WallpaperDashboard");
    let state_path = local.join("desktop-panel-placement.json");
    let saved: Option<Placement> = fs::read(&state_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let parent = desktop_host()?;
    let mut event_loop = EventLoopBuilder::<PanelMessage>::with_user_event().build();
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let primary = event_loop.primary_monitor();
    let previous = saved.as_ref().and_then(|placement| {
        monitors
            .iter()
            .find(|monitor| monitor_id(monitor) == placement.monitor)
    });
    let monitor = previous
        .or_else(|| {
            monitors
                .iter()
                .find(|monitor| Some(*monitor) != primary.as_ref())
        })
        .or(primary.as_ref())
        .or(monitors.first())
        .ok_or_else(|| io::Error::other("找不到可用的显示器"))?;
    let window = WindowBuilder::new()
        .with_title("硬件仪表盘 · 副屏面板")
        .with_window_classname(CLASS_NAME)
        .with_parent_window(parent as isize)
        .with_decorations(false)
        .with_resizable(false)
        .with_inner_size(PhysicalSize::new(640, 520))
        .with_visible(false)
        .build(&event_loop)?;
    // SAFETY: The child window belongs to the current UI thread.
    if unsafe { SetWindowSubclass(window.hwnd() as HWND, Some(borderless_child), 1, 0) } == 0 {
        return Err(io::Error::other("无法设置桌面面板边界").into());
    }
    let mut placement = position_window(&window, parent, monitor, previous.and(saved.as_ref()))?;
    apply_region(&window, [1.0, 1.0], &[])?;
    let events = event_loop.create_proxy();
    let key = serde_json::to_string(&shortcuts::dock_pairing_key()?)?;
    let mut context = WebContext::new(Some(local.join("WebView2-Panel")));
    let _webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(PANEL_URL)
        .with_initialization_script(format!(
            "window.wallpaperDashboardPanel = true; window.wallpaperShortcutKey = {key};"
        ))
        .with_navigation_handler(|url| url == PANEL_URL)
        .with_ipc_handler(move |request| {
            if request.uri() == PANEL_URL
                && request.body().len() <= 8192
                && let Ok(message) = serde_json::from_str(request.body())
            {
                let _ = events.send_event(message);
            }
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_background_color((15, 21, 34, 255))
        .with_default_context_menus(false)
        .with_browser_accelerator_keys(false)
        .with_back_forward_navigation_gestures(false)
        .build(&window)?;

    window.set_visible(true);
    let hwnd = window.hwnd() as HWND;
    // SAFETY: hwnd is a child window owned by this thread. If layered child
    // composition is unavailable, retain the normal clipped panel surface.
    unsafe {
        // Tao registers a private DC for this class; Win32 layered windows
        // require the standard shared DC instead.
        let class_style = GetClassLongPtrW(hwnd, GCL_STYLE);
        SetClassLongPtrW(
            hwnd,
            GCL_STYLE,
            (class_style & !(CS_OWNDC as usize)) as isize,
        );
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED as isize);
        if SetLayeredWindowAttributes(hwnd, 0, 179, LWA_ALPHA) == 0 {
            collector_log::record(&format!(
                "desktop panel layer composition: {}",
                io::Error::last_os_error()
            ));
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            SetClassLongPtrW(hwnd, GCL_STYLE, class_style as isize);
        }
    }

    let mut stored = saved;
    let mut pending = None;
    let mut shape = None;
    event_loop.run_return(|event, _, flow| {
        let mut exiting = false;
        match event {
            Event::UserEvent(PanelMessage::LaunchApp { request_id, target }) => {
                let error = launch_app(hwnd, &target).err();
                let response = serde_json::json!({ "requestId": request_id, "error": error });
                let _ =
                    _webview.evaluate_script(&format!("window.finishPanelAppLaunch?.({response})"));
            }
            Event::UserEvent(PanelMessage::Region { viewport, cards }) => {
                if let Err(error) = apply_region(&window, viewport, &cards) {
                    collector_log::record(&format!("could not set desktop panel shape: {error}"));
                }
                shape = Some((viewport, cards));
            }
            Event::UserEvent(PanelMessage::Move { delta: [dx, dy] }) => {
                if let Some(monitor) = window.current_monitor() {
                    capture_placement(&window, &mut placement);
                    let scale = window.scale_factor();
                    placement.x = placement.x.saturating_add((dx * scale) as i32);
                    placement.y = placement.y.saturating_add((dy * scale) as i32);
                    if let Ok(next) = position_window(&window, parent, &monitor, Some(&placement)) {
                        placement = next;
                        if let Some((viewport, cards)) = &shape {
                            let _ = apply_region(&window, *viewport, cards);
                        }
                    }
                    pending = Some(Instant::now());
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                *flow = ControlFlow::Exit;
                return;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                capture_placement(&window, &mut placement);
                exiting = true;
            }
            Event::WindowEvent {
                event:
                    WindowEvent::Moved(_)
                    | WindowEvent::Resized(_)
                    | WindowEvent::ScaleFactorChanged { .. },
                ..
            } => {
                pending = Some(Instant::now() + Duration::from_millis(350));
            }
            _ => {}
        }
        if exiting || pending.is_some_and(|time| time <= Instant::now()) {
            pending = None;
            capture_placement(&window, &mut placement);
            if stored.as_ref() != Some(&placement) {
                match save_placement(&state_path, &placement) {
                    Ok(()) => stored = Some(placement.clone()),
                    Err(error) => {
                        collector_log::record(&format!("could not save panel placement: {error}"))
                    }
                }
            }
        }
        *flow = if exiting {
            ControlFlow::Exit
        } else {
            pending.map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
        };
    });
    Ok(())
}
