# Sensor probe

Run from the repository root on Windows:

```powershell
cargo run --locked -- --json
```

Use an administrator terminal to include MSI Center motherboard fans and the
Ryzen Master SDK telemetry, when those providers are installed. The default
JSON command uses the existing Windows and vendor interfaces. The optional
`--lpcio PATH_TO_LpcIO.bin` and `--presentmon PATH_TO_EXE PID` commands are
separate text diagnostics.

## Snapshot schema v1

The root object contains `schema_version` and a list of `backends`. Each backend
names its `source`, records `sampled_at_unix_ms`, and returns either devices or a
status and diagnostic detail. The timestamp is when the backend completed the
sample. Each device has an `id`, a display `name`, a `kind`, and its sensors.
A sensor has an `id`, a display `label`, a `unit`, a numeric or null `value`,
and a `status`. Missing sensors and null values should both render as unknown.

- Use `source + device.id + sensor.id` to identify a reading. Matching IDs across
  sources can describe the same physical device, as with `cpu:system`.
- Use IDs for selection and labels for display. Fallback IDs containing
  `index` can change when hardware is added or its enumeration order changes.
- `available` can include zero. `unsupported`, `permission_denied`, and
  `unavailable` accompany null readings or an empty device list.
- Temperatures use °C, clocks MHz, power W, voltage V, and capacities bytes (B).
  NVIDIA fan target readings have their own IDs so they remain distinguishable
  from tachometer measurements.

## Verification

Windows, AMD ADLX, AMD Ryzen Master SDK, and MSI Center were verified on a
Ryzen 9 9950X, Radeon RX 7700 XT, and MSI B650M GAMING PLUS WIFI. NVIDIA NVML
compiles and reports unsupported on this host. Its per-device readings still
need verification on a Windows computer with an NVIDIA card. MSI Center
uses internal shared memory and is only verified on this installation.


## Live dashboard

Start the continuous collector in PowerShell:

```powershell
cargo run --locked -- --serve
```

Open http://127.0.0.1:8766/ for the local diagnostics page. For Ryzen SDK
temperatures and MSI Center motherboard RPM, launch the collector from an
administrator terminal. GPU selection and fan labels on the diagnostics page
are stored in the browser's local storage.

The service binds exclusively to IPv4 loopback and exposes read-only
snapshots. Windows, ADLX, and NVML sample every two seconds, MSI Center samples every second, and the Ryzen SDK samples every four seconds. Each worker updates independently.
Windows disk capacity and free space refresh every ten seconds.
The dashboard hides stale numbers until fresh snapshots arrive.

## Windows tray executable

Build the tray executable from the repository root:

```powershell
cargo build --release --locked --bin wallpaper-dashboard
```

Launch `target/release/wallpaper-dashboard.exe` from the repository root.
Choose **设置** in its tray menu to open the desktop settings window.
It uses the Microsoft Edge WebView2 Runtime and runs as the desktop user,
even when the collector runs as administrator. Closing it leaves collection
running. The same page is also available at http://127.0.0.1:8766/settings.
The page includes weather, live collector status and the shortcut manager.
The notification-area menu has an **退出采集器** item. Exit any existing collector
before launching another instance. For AMD CPU temperatures and MSI motherboard
fans, right-click the executable and choose **Run as administrator**.

Failed sampling transitions and recoveries are logged to
`%LOCALAPPDATA%\WallpaperDashboard\collector.log`. Each entry starts with a
Unix timestamp in milliseconds and includes the source and elapsed sampling
time for failures. Log rotation retains one prior file. The loopback snapshot
also returns `sample_duration_ms` and an optional `last_error`.
Transient failures retain the previous sample for at most 12 seconds for
the Ryzen SDK or 7 seconds for the other sources. Its original timestamp
is retained and the dashboard labels it. Permission failures invalidate
existing readings immediately.

## Configure the dashboard shortcuts

With the collector running, open http://127.0.0.1:8766/shortcuts. Search the
Start-menu and Desktop shortcuts or locally installed Steam titles. Add entries
to either row, reorder them, and save. The app and game rows hold up to 11 and
7 entries. The desktop panel checks for layout and icon changes with its normal
sensor refresh and checks periodically for changes in the Steam catalog.
Detected app icons can be replaced with a local image.

Each row shows pinned entries first. Empty application slots are filled from
the most recently used foreground Windows applications that match known shortcuts.
The collector keeps up to 32 recent shortcut IDs locally. Empty game slots are filled using last-played
records from installed Steam games. The game row currently uses the Steam catalog.

Open **显示 / 隐藏桌面面板** from the tray to display the shortcuts. The desktop
panel is built from assets in `ui/panel` and uses a local collector connection.
Launch requests are limited to matched shortcuts and open through Windows Explorer.

## Show game FPS in the desktop panel

Place the x64 console executable from [PresentMon 2.6.0](https://github.com/GameTechDev/PresentMon/releases/tag/v2.6.0)
beside the collector as `PresentMon.exe`. The collector checks this release's
SHA-256 and starts FPS tracking itself. No additional command or PID is needed.
Use an administrator session if PresentMon cannot start the ETW capture.
Tracking follows the foreground executable when it is in an installed Steam
game folder or is the direct executable target of a Windows shortcut bound
to the game row. Other sensors still work if `PresentMon.exe` is absent.

For games started by launchers whose shortcut points at a different executable,
the manual PID mode is still available:

```powershell
cargo run --release --locked -- --track-fps PATH_TO_PRESENTMON.EXE GAME_PID
```

The tracker publishes presented FPS and mean frame time in five-second
windows. It reports 1% low when the selected swap chain has at least 100
intervals. In automatic mode, the desktop panel clears the reading when focus
moves away from the game. Expired samples are also cleared. The integrated
tracker exits with the collector. Stop the manual CLI tracker with Ctrl+C.

## Peripheral battery and weather

The battery panel shows percentages exposed by Windows device properties.
It also reads MCHOSE `41E4:2001` and NearLink `373B:10C9` receivers directly
over USB HID every 30 seconds. The collector sends only battery queries on
matching vendor collections and omits devices that do not reply. The mouse
also reports whether it is on external power. No vendor utility is required.
The protocols are documented by [mchose-battery-tray](https://github.com/marsuyone/mchose-battery-tray)
and [HaloBattery](https://github.com/HeyOkay/HaloBattery/pull/347).

Manual weather cities use Open-Meteo geocoding. Automatic location uses IPWhois
over a direct connection and refuses an obvious time-zone mismatch. The location
test does not save changes. VPN or TUN routes can still affect IP location.
Forecast and air-quality sources are [Open-Meteo](https://open-meteo.com/) and
[CAMS](https://open-meteo.com/en/docs/air-quality-api), respectively.

The build uses its own panel source, generated SVG weather icons, and system
fonts. Wallpaper Engine can independently display the underlying desktop
wallpaper. It is not a runtime or build dependency of the native panel.

## UI provenance

The native panel layout and parts of its HTML, CSS, and JavaScript were adapted
from [Apple风格副屏 (Steam Workshop item 3833992898)](https://steamcommunity.com/sharedfiles/filedetails/?id=3833992898).
The original reference snapshot, artwork, fonts, and Git history are outside
this repository. This note records the source of the adapted code.
