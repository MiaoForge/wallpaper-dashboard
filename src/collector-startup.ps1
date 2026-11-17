$ErrorActionPreference = 'Stop'
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$name = "WallpaperDashboardCollector-$sid"
$task = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
switch ($env:WALLPAPER_DASHBOARD_STARTUP) {
 'enable' {
  $action = New-ScheduledTaskAction -Execute $env:WALLPAPER_DASHBOARD_EXE
  $trigger = New-ScheduledTaskTrigger -AtLogOn -User $sid
  $principal = New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Highest
  $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
  Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
  $task = Get-ScheduledTask -TaskName $name
 }
 'disable' {
  if ($task) { Unregister-ScheduledTask -TaskName $name -Confirm:$false }
  $task = $null
 }
}
if ($task -and $task.State -ne 'Disabled') { 'enabled' } else { 'disabled' }
