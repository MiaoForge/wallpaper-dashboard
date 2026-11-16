$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$shell = New-Object -ComObject WScript.Shell
foreach ($path in ConvertFrom-Json -InputObject $env:WALLPAPER_DASHBOARD_GAME_LINKS) {
    try {
        $target = $shell.CreateShortcut($path).TargetPath
        if ($target -and [IO.Path]::GetExtension($target) -ieq '.exe' -and $target -match '^[a-zA-Z]:[\\/]') {
            $target
        }
    } catch {}
}
