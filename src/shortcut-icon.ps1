$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($env:WALLPAPER_DASHBOARD_ICON_LINK)
$target = $shortcut.TargetPath
if (!$target -or $target.StartsWith('\\') -or !(Test-Path -LiteralPath $target -PathType Leaf)) { exit 1 }
$path = $target
$index = 0
if ($shortcut.IconLocation -match '^(.*),\s*(-?\d+)$') {
 $candidate = [Environment]::ExpandEnvironmentVariables($Matches[1].Trim().Trim('"'))
 if ($candidate -and !$candidate.StartsWith('\\') -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
  $path = $candidate
  $index = [int]$Matches[2]
 }
}
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ShortcutIcons {
 [DllImport("user32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
 public static extern uint PrivateExtractIconsW(string path, int index, int cx, int cy, out IntPtr icon, out uint id, uint count, uint flags);
 [DllImport("user32.dll")]
 public static extern bool DestroyIcon(IntPtr icon);
}
'@
$handle = [IntPtr]::Zero
$resource = [uint32]0
$icon = $null
$bitmap = $null
$stream = New-Object IO.MemoryStream
try {
 $count = [ShortcutIcons]::PrivateExtractIconsW($path, $index, 256, 256, [ref]$handle, [ref]$resource, 1, 0)
 if ($count -eq 1 -and $handle -ne [IntPtr]::Zero) {
  $icon = [System.Drawing.Icon]::FromHandle($handle)
 } else {
  $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($target)
 }
 if (!$icon) { exit 1 }
 $bitmap = $icon.ToBitmap()
 $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
 [Console]::Write([Convert]::ToBase64String($stream.ToArray()))
} finally {
 $stream.Dispose()
 if ($bitmap) { $bitmap.Dispose() }
 if ($icon) { $icon.Dispose() }
 if ($handle -ne [IntPtr]::Zero) { [void][ShortcutIcons]::DestroyIcon($handle) }
}
