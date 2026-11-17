use std::{
    io,
    os::windows::io::{FromRawHandle, OwnedHandle},
    path::PathBuf,
    ptr, thread,
    time::Duration,
};

use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::{run_return::EventLoopExtRunReturn, windows::WindowBuilderExtWindows},
    window::WindowBuilder,
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindowAsync,
    },
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder, WebViewBuilderExtWindows};

const CLASS_NAME: &str = "WallpaperDashboardSettingsWindow";
const SETTINGS_URL: &str = "http://127.0.0.1:8766/settings";

fn focus_existing(class_name: &str) {
    let class: Vec<_> = class_name.encode_utf16().chain([0]).collect();
    // A second launch may arrive while WebView2 is still initializing.
    for _ in 0..20 {
        // SAFETY: class is NUL-terminated and is retained throughout the calls.
        unsafe {
            let window = FindWindowW(class.as_ptr(), ptr::null());
            if !window.is_null() {
                if IsIconic(window) != 0 {
                    ShowWindowAsync(window, SW_RESTORE);
                }
                SetForegroundWindow(window);
                return;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn claim_window(class_name: &str) -> io::Result<Option<OwnedHandle>> {
    let name: Vec<_> = format!(r"Local\{class_name}")
        .encode_utf16()
        .chain([0])
        .collect();
    // SAFETY: name is NUL-terminated. A successful handle is owned below.
    let (handle, exists) = unsafe {
        let handle = CreateMutexW(ptr::null(), 0, name.as_ptr());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        (
            OwnedHandle::from_raw_handle(handle),
            GetLastError() == ERROR_ALREADY_EXISTS,
        )
    };
    if exists {
        focus_existing(class_name);
        return Ok(None);
    }
    Ok(Some(handle))
}

pub(super) fn run() -> Result<(), super::Error> {
    let Some(_instance) = claim_window(CLASS_NAME)? else {
        return Ok(());
    };
    let profile = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("无法读取本地应用数据目录"))?
        .join("WallpaperDashboard")
        .join("WebView2");
    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("壁纸仪表盘设置")
        .with_window_classname(CLASS_NAME)
        .with_inner_size(LogicalSize::new(850.0, 570.0))
        .with_min_inner_size(LogicalSize::new(430.0, 400.0))
        .with_visible(false)
        .build(&event_loop)?;
    let mut context = WebContext::new(Some(profile));
    let _webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(SETTINGS_URL)
        .with_initialization_script("window.wallpaperDashboardDesktop = true")
        .with_navigation_handler(|url| url.split('#').next() == Some(SETTINGS_URL))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_default_context_menus(false)
        .with_browser_accelerator_keys(false)
        .build(&window)
        .map_err(|error| {
            format!("无法初始化设置窗口。请检查 Microsoft Edge WebView2 Runtime。\n{error}")
        })?;
    window.set_visible(true);
    window.set_focus();
    event_loop.run_return(|event, _, flow| {
        *flow = match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => ControlFlow::Exit,
            _ => ControlFlow::Wait,
        };
    });
    Ok(())
}
