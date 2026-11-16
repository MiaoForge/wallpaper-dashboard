# Wallpaper Dashboard

A Windows desktop panel and Rust hardware collector for a secondary display.
The panel runs from the notification area and shows live system metrics,
weather, connected devices, and app and game shortcuts. Windows and Wallpaper
Engine can provide the desktop background independently of the panel.

## Build and run

Install the Rust toolchain and Microsoft Edge WebView2 Runtime. Build from the
repository root in PowerShell:

```powershell
cargo build --release --locked --manifest-path sensor-probe/Cargo.toml --bin wallpaper-dashboard
```

Start `sensor-probe/target/release/wallpaper-dashboard.exe`. Open **设置** or
**显示 / 隐藏桌面面板** from its notification-area menu. If another copy of the
collector is running, exit it before starting this build.

See the [collector guide](sensor-probe/README.md) for hardware requirements,
optional FPS tracking, diagnostics, and the origin of the adapted panel UI.
