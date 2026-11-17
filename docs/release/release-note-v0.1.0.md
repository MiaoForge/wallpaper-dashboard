## First public release

Wallpaper Dashboard is a Windows hardware dashboard designed for a secondary
display. Open the desktop panel and settings from the notification area.

- Monitor CPU, GPU, memory, disk, network, fan speeds, and supported
  peripheral batteries. Choose which network adapter the panel displays.
- See the weather and air quality for a selected city, or use automatic
  location.
- Pin apps and games to the panel. Available slots can show recently used
  apps and recently played Steam games.
- Configure Windows startup, what closes when the collector exits, and
  manually check for future releases in **常规**.

## Install on Windows

Extract `wallpaper-dashboard-v0.1.0-windows-x64.zip` and run
`wallpaper-dashboard.exe`. The panel and settings window require Microsoft
Edge WebView2 Runtime. Run the tray collector as administrator for supported
AMD Ryzen SDK temperature and MSI Center motherboard fan readings.

Game FPS tracking requires a separate PresentMon executable. See the
[collector guide](https://github.com/MiaoForge/wallpaper-dashboard/blob/v0.1.0/docs/collector.md)
for supported hardware and setup.
