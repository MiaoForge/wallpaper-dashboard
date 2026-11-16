$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$apps=[Collections.Generic.List[object]]::new()
Get-StartApps | Where-Object AppID -Match '^[a-zA-Z0-9._-]+![a-zA-Z0-9._-]+$' | ForEach-Object { $apps.Add(@($_.Name, $_.AppID)) }
ConvertTo-Json -InputObject $apps.ToArray() -Depth 3 -Compress
