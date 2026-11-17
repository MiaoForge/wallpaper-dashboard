$ErrorActionPreference = 'Stop'
$loc = 0
$root = 0
$hwnd = 0
$desktop = (New-Object -ComObject Shell.Application).Windows().FindWindowSW([ref]$loc, [ref]$root, 8, [ref]$hwnd, 1)
if (!$desktop -or !$desktop.Document.Application) { throw 'Explorer desktop is unavailable' }
$desktop.Document.Application.ShellExecute($env:WALLPAPER_DASHBOARD_SHORTCUT, $env:WALLPAPER_DASHBOARD_ARGUMENTS, '', 'open', 1)
