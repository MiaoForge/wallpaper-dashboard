$ErrorActionPreference = 'Stop'
$inputStream = New-Object IO.StreamReader([Console]::OpenStandardInput(), [Text.Encoding]::UTF8)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$paths = ConvertFrom-Json -InputObject $inputStream.ReadToEnd()
$shell = New-Object -ComObject WScript.Shell
$results = [Collections.Generic.List[object]]::new()
for ($index = 0; $index -lt $paths.Count; $index++) {
    try {
        $target = $shell.CreateShortcut([string]$paths[$index]).TargetPath
        if ($target -match '^[a-zA-Z]:[\\/]' -and [IO.Path]::GetExtension($target) -ieq '.exe') {
            $results.Add(@($index, $target))
        }
    } catch {}
}
ConvertTo-Json -InputObject $results.ToArray() -Depth 3 -Compress
