$ErrorActionPreference = 'Stop'
$family, $appId = $env:WALLPAPER_DASHBOARD_APP_ID -split '!', 2
$package = Get-AppxPackage | Where-Object PackageFamilyName -EQ $family | Select-Object -First 1
if (!$package) { exit 1 }
$app = (Get-AppxPackageManifest $package).Package.Applications.Application | Where-Object Id -EQ $appId | Select-Object -First 1
$logo = [string]$app.VisualElements.Square44x44Logo
if (!$logo -or [IO.Path]::IsPathRooted($logo)) { exit 1 }
$root = [IO.Path]::GetFullPath($package.InstallLocation).TrimEnd('\') + '\'
$path = [IO.Path]::GetFullPath((Join-Path $root $logo))
if (!$path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { exit 1 }
$stem = [IO.Path]::GetFileNameWithoutExtension($path)
$qualifier = '^' + [regex]::Escape($stem) + '(?:\.(targetsize|scale)-(\d+)(?:_altform-(unplated))?)?\.png$'
$images = Get-ChildItem -LiteralPath ([IO.Path]::GetDirectoryName($path)) -File | ForEach-Object {
    if ($_.Length -le 280KB -and $_.Name -match $qualifier) {
        $size = switch ($Matches[1]) {
            'targetsize' { [int]$Matches[2] }
            'scale' { 44 * [int]$Matches[2] / 100 }
            default { 44 }
        }
        [pscustomobject]@{ Path = $_.FullName; Size = $size; Unplated = [int]($Matches[3] -eq 'unplated') }
    }
}
$best = $images | Sort-Object -Property Size, Unplated -Descending | Select-Object -First 1
if (!$best) { exit 1 }
[Console]::Write([Convert]::ToBase64String([IO.File]::ReadAllBytes($best.Path)))
