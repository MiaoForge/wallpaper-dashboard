#![windows_subsystem = "windows"]

#[path = "../panel_window.rs"]
mod panel_window;
#[path = "../settings_window.rs"]
mod settings_window;

use std::{
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use sensor_probe::{collector_log, service::Collector};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
};
use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MB_ICONERROR, MSG, MessageBoxW, PM_NOREMOVE, PeekMessageW,
        PostThreadMessageW, TranslateMessage, WM_APP,
    },
};

const SERVICE_STOPPED: u32 = WM_APP + 1;
type Error = Box<dyn std::error::Error + Send + Sync>;

fn show_error(error: &str) {
    let title: Vec<u16> = "硬件仪表盘".encode_utf16().chain([0]).collect();
    let error: Vec<u16> = error.encode_utf16().chain([0]).collect();
    // SAFETY: Both strings are NUL-terminated and live during the call.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            error.as_ptr(),
            title.as_ptr(),
            MB_ICONERROR,
        )
    };
}

fn icon() -> Result<Icon, tray_icon::BadIcon> {
    let mut rgba = Vec::with_capacity(32 * 32 * 4);
    for y in 0_i32..32 {
        for x in 0_i32..32 {
            let color = if !(2..30).contains(&x) || !(2..30).contains(&y) {
                [0, 0, 0, 0]
            } else if (7..25).contains(&x) && (y - (24 - (x - 7) * 3 / 4)).abs() <= 2 {
                [131, 243, 183, 255]
            } else {
                [22, 31, 28, 255]
            };
            rgba.extend_from_slice(&color);
        }
    }
    Icon::from_rgba(rgba, 32, 32)
}

fn run() -> Result<(), Error> {
    let collector = Collector::bind().map_err(|error| {
        format!("采集器无法监听 127.0.0.1:8766。请检查是否已有采集器在运行。\n{error}")
    })?;
    let menu = Menu::new();
    let panel = MenuItem::new("显示 / 隐藏桌面面板", true, None);
    let settings = MenuItem::new("设置", true, None);
    let game_mode = CheckMenuItem::new("游戏兼容模式（暂停 GPU 采集）", true, false, None);
    let quit = MenuItem::new("退出采集器", true, None);
    menu.append(&panel)?;
    menu.append(&settings)?;
    menu.append(&game_mode)?;
    menu.append(&quit)?;
    let _tray = TrayIconBuilder::new()
        .with_tooltip("硬件仪表盘采集器")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_icon(icon()?)
        .build()?;
    let stop = AtomicBool::new(false);
    let mut msg = MSG::default();
    // SAFETY: Create the current thread's message queue before the worker starts.
    let main_thread = unsafe {
        PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_NOREMOVE);
        GetCurrentThreadId()
    };

    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let result = collector.serve(&stop);
            // SAFETY: The ID is from the main thread and remains live until join.
            unsafe { PostThreadMessageW(main_thread, SERVICE_STOPPED, 0, 0) };
            result
        });
        'messages: loop {
            // SAFETY: msg points to writable storage and this thread owns the queue.
            let status = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
            if status <= 0 || msg.message == SERVICE_STOPPED {
                break;
            }
            // SAFETY: The message was retrieved from this thread's queue.
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            for event in MenuEvent::receiver().try_iter() {
                if event.id == *quit.id() {
                    break 'messages;
                }
                if event.id == *game_mode.id() {
                    sensor_probe::game_mode::set_manual(game_mode.is_checked());
                    continue;
                }
                let open = if event.id == *panel.id() {
                    sensor_probe::shortcuts::open_panel
                } else if event.id == *settings.id() {
                    sensor_probe::shortcuts::open_settings
                } else {
                    continue;
                };
                thread::spawn(move || {
                    if let Err(error) = open() {
                        collector_log::record(&format!("could not open window: {error}"));
                        show_error("无法打开仪表盘窗口。");
                    }
                });
            }
        }
        stop.store(true, Ordering::Release);
        worker.join().map_err(|_| "collector thread panicked")?
    })
}

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("--settings-window") => settings_window::run(),
        Some("--panel-window") => panel_window::run(),
        Some("--toggle-panel") => panel_window::toggle(),
        _ => run(),
    };
    if let Err(error) = result {
        collector_log::record(&format!("startup or service error: {error}"));
        show_error(&error.to_string());
        std::process::exit(1);
    }
}
