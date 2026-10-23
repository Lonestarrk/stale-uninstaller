# Elevated install/launch/uninstall smoke test. Copy to .superpowers/elevated/jobs/NNN-installer-X.Y.Z.ps1,
# set $version below, then run: powershell -NoProfile -Command "schtasks /run /tn StaleDevElevated".
$version = 'X.Y.Z'
. 'H:\Projects\uninstaller\.superpowers\elevated\lib.ps1'
$ErrorActionPreference = 'Continue'
$data = 'C:\ProgramData\Stale Uninstaller'
$app = "$env:ProgramFiles\Stale Uninstaller"
$setup = "H:\Projects\uninstaller\installer\Output\StaleUninstaller-Setup-$version.exe"
$ts = Get-Date -Format yyyyMMdd-HHmmss
$backup = "H:\Projects\uninstaller\.superpowers\elevated\backup-$ts"
$logs = "H:\Projects\uninstaller\.superpowers\elevated\out"
$uninstKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{006E9CB5-0F91-495D-A821-CB149DD3217F}_is1'
"setup sha256 $((Get-FileHash $setup).Hash) size $((Get-Item $setup).Length)"
"before: app folder exists=$(Test-Path $app) uninstall key exists=$(Test-Path $uninstKey) data exists=$(Test-Path $data)"
if (Get-Process StaleUninstaller -ErrorAction SilentlyContinue) { "ABORT: Stale Uninstaller is running"; return }

# (a) Quarantine manifests: any manifest folder at all (trusted or not) means the uninstall half is skipped.
$all = @(); $trusted = @()
if (Test-Path "$data\quarantine") {
  foreach ($d in Get-ChildItem -Force -Directory "$data\quarantine") {
    if (-not (Test-Path (Join-Path $d.FullName 'manifest.json'))) { continue }
    $all += $d.Name
    $isLink = ($d.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0
    $owner = (Get-Acl $d.FullName).GetOwner([Security.Principal.SecurityIdentifier]).Value
    if (-not $isLink -and ($owner -eq 'S-1-5-32-544' -or $owner -eq 'S-1-5-18')) { $trusted += $d.Name }
  }
}
"quarantine manifests: $($all.Count) (trusted $($trusted.Count))"
# Any manifest also skips the launch: starting Stale runs the automatic purge of old quarantines.
$runUninstall = ($all.Count -eq 0)

# (b) Back up the data folder with owner and ACLs.
$rootSddl = $null
$before = @()
if (Test-Path $data) {
  $rootSddl = (Get-Acl $data).Sddl
  $before = Get-ChildItem -Force -Recurse $data | ForEach-Object { $_.FullName.Substring($data.Length) } | Sort-Object
  robocopy $data $backup /E /B /COPY:DATSO /DCOPY:DAT /XJ /R:0 /W:0 /NP /NFL /NDL | Select-Object -Last 12
  "backup robocopy exit=$LASTEXITCODE entries=$($before.Count)"
  if ($LASTEXITCODE -ge 8) { "ABORT: backup failed"; return }
}

# Install.
$p = Start-Process -FilePath $setup -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/LOG=$logs\setup-install-$version.log" -Wait -PassThru
"install exit=$($p.ExitCode)"
"after install: exe exists=$(Test-Path "$app\StaleUninstaller.exe") unins000 exists=$(Test-Path "$app\unins000.exe") uninstall key exists=$(Test-Path $uninstKey)"
if (Test-Path $uninstKey) { $k = Get-ItemProperty $uninstKey; "uninstall entry: DisplayName=$($k.DisplayName) DisplayVersion=$($k.DisplayVersion) DisplayIcon=$($k.DisplayIcon)" }
$startMenu = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Stale Uninstaller.lnk"
"start menu shortcut exists=$(Test-Path $startMenu)"
"installed exe version=$((Get-Item "$app\StaleUninstaller.exe").VersionInfo.FileVersion)"

# Launch the installed program and take a screenshot (only with an empty quarantine).
if ($runUninstall) {
  $proc = Start-Process -FilePath "$app\StaleUninstaller.exe" -PassThru
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $h = [G]::MainWindow([uint32]$proc.Id) }
  "launched pid $($proc.Id) hwnd $h"
  Start-Sleep -Seconds 5
  if ($h -ne [IntPtr]::Zero) { [void][G]::ShowWindow($h, 9); Save-Shot $h "installer-$version" }
  $ic = [System.Drawing.Icon]::ExtractAssociatedIcon("$app\StaleUninstaller.exe"); $ic.ToBitmap().Save("H:\Projects\uninstaller\.superpowers\elevated\shots\installer-$version-icon.png"); "icon extracted $($ic.Width)x$($ic.Height)"
  $ev = Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName='SideBySide';StartTime=(Get-Date).AddMinutes(-5)} -ErrorAction SilentlyContinue; "sidebyside events: $(@($ev).Count)"
  "proc alive=$(-not $proc.HasExited) elevated-check path=$((Get-Process -Id $proc.Id -ErrorAction SilentlyContinue).Path)"
  "hives folder while running exists=$(Test-Path 'C:\ProgramData\Stale Uninstaller\hives') children=$(@(Get-ChildItem -Force 'C:\ProgramData\Stale Uninstaller\hives' -ErrorAction SilentlyContinue).Count)"; Stop-Stale @{ Proc = $proc }
  Start-Sleep -Seconds 2
} else {
  "LAUNCH SKIPPED: the quarantine holds $($all.Count) manifest(s)"
}

if ($runUninstall) {
  $a = Start-Process -FilePath "$app\StaleUninstaller.exe" -ArgumentList '--uninstall-cleanup','--ask','--silent' -Wait -PassThru
  "direct --ask --silent exit=$($a.ExitCode) data folder still exists=$(Test-Path $data)"
  $u = Start-Process -FilePath "$app\unins000.exe" -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART',"/LOG=$logs\setup-uninstall-$version.log" -Wait -PassThru
  "uninstall exit=$($u.ExitCode)"
  for ($i = 0; $i -lt 60 -and (Test-Path $app); $i++) { Start-Sleep -Milliseconds 500 }
  "after uninstall: app folder exists=$(Test-Path $app) uninstall key exists=$(Test-Path $uninstKey) start menu shortcut exists=$(Test-Path $startMenu) data folder exists=$(Test-Path $data)"
  "C:\Stale Uninstaller exists=$(Test-Path 'C:\Stale Uninstaller')"
  Select-String -Path "$logs\setup-uninstall-$version.log" -Pattern 'Stale cleanup|Uninstall step|Starting the uninstallation|Deleting' | ForEach-Object { "uninstall log: $($_.Line)" }
} else {
  "UNINSTALL SKIPPED: the quarantine holds $($all.Count) manifest(s); the program stays installed"
}

# (c) Restore the data folder with owner and ACLs.
if (Test-Path $backup) {
  robocopy $backup $data /E /B /COPY:DATSO /DCOPY:DAT /XJ /R:0 /W:0 /NP /NFL /NDL | Select-Object -Last 12
  "restore robocopy exit=$LASTEXITCODE"
  if ($rootSddl) { $acl = Get-Acl $data; $acl.SetSecurityDescriptorSddlForm($rootSddl); Set-Acl -Path $data -AclObject $acl }
  "root sddl restored equal=$((Get-Acl $data).Sddl -eq $rootSddl)"
  $after = Get-ChildItem -Force -Recurse $data | ForEach-Object { $_.FullName.Substring($data.Length) } | Sort-Object
  $missing = @($before | Where-Object { $after -notcontains $_ })
  "entries before=$($before.Count) after=$($after.Count) missing=$($missing.Count) $($missing -join ', ')"
  foreach ($f in 'settings.json','important.json') { "$f present=$(Test-Path "$data\$f") equal=$(if (Test-Path "$backup\$f") { (Get-FileHash "$data\$f").Hash -eq (Get-FileHash "$backup\$f").Hash } else { 'no-backup' })" }
  }



