; Inno Setup script for Stale Uninstaller.
; Build the release exe first (cargo build -p stale-gui --release), then:
;   "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" /DAppVersion=X.Y.Z installer\stale.iss
; The setup lands in installer\Output\StaleUninstaller-Setup-X.Y.Z.exe.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=X.Y.Z stale.iss
#endif

[Setup]
; Never change the AppId: Windows recognises upgrades and the uninstall entry by it.
AppId={{006E9CB5-0F91-495D-A821-CB149DD3217F}
AppName=Stale Uninstaller
AppVersion={#AppVersion}
AppVerName=Stale Uninstaller {#AppVersion}
AppPublisher=Jimmy Revtorp
AppPublisherURL=https://github.com/Lonestarrk/stale-uninstaller
AppSupportURL=https://github.com/Lonestarrk/stale-uninstaller/issues
AppUpdatesURL=https://github.com/Lonestarrk/stale-uninstaller/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Stale Uninstaller
DefaultGroupName=Stale Uninstaller
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputBaseFilename=StaleUninstaller-Setup-{#AppVersion}
LicenseFile=..\LICENSE
SetupIconFile=..\crates\stale-gui\stale.ico
UninstallDisplayIcon={app}\StaleUninstaller.exe
UninstallDisplayName=Stale Uninstaller
; A running Stale Uninstaller holds this mutex (main.rs, INSTANCE_MUTEX): setup and uninstall
; ask the user to close it first.
AppMutex=Global\StaleUninstaller
CloseApplications=yes
ShowLanguageDialog=auto
WizardStyle=modern
Compression=lzma2
SolidCompression=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "swedish"; MessagesFile: "compiler:Languages\Swedish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\StaleUninstaller.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Stale Uninstaller"; Filename: "{app}\StaleUninstaller.exe"
Name: "{autodesktop}\Stale Uninstaller"; Filename: "{app}\StaleUninstaller.exe"; Tasks: desktopicon

[Run]
; shellexec: the exe asks for elevation itself (its manifest), which CreateProcess would refuse
; from a non-elevated post-install step.
Filename: "{app}\StaleUninstaller.exe"; Description: "{cm:LaunchProgram,Stale Uninstaller}"; Flags: nowait postinstall skipifsilent shellexec

[Code]
// Stale's own cleanup of its quarantine and %ProgramData%\Stale Uninstaller runs in two phases,
// because InitializeUninstall comes before the uninstaller's "Are you sure…?" confirmation:
//  1. InitializeUninstall: StaleUninstaller.exe --uninstall-cleanup --ask only decides (and asks
//     about a quarantine). Exit code 10 = remove the data folder, 11 = purge then remove (the user
//     chose Yes), 12 = keep everything, 2 = cancel the uninstall. A silent uninstall adds --silent:
//     nobody is asked and a quarantine is kept. If the exe cannot run, everything is kept.
//  2. CurUninstallStepChanged(usUninstall), after the confirmation and before any file is removed:
//     for 10 or 11, --apply remove|purge does the work (it re-checks the quarantine and never
//     purges one it was not asked about).
var
  CleanupCode: Integer;

function CleanupParams(Phase: String): String;
begin
  Result := '--uninstall-cleanup ' + Phase;
  if UninstallSilent then
    Result := Result + ' --silent';
end;

function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  CleanupCode := 12;
  if Exec(ExpandConstant('{app}\StaleUninstaller.exe'), CleanupParams('--ask'), '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode) then
  begin
    Log(Format('Stale cleanup --ask exit code: %d', [ResultCode]));
    if ResultCode = 2 then
      Result := False
    else if (ResultCode = 10) or (ResultCode = 11) then
      CleanupCode := ResultCode;
  end
  else
    Log('Could not run the Stale cleanup: ' + SysErrorMessage(ResultCode));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
  Mode: String;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  if CleanupCode = 10 then
    Mode := 'remove'
  else if CleanupCode = 11 then
    Mode := 'purge'
  else
  begin
    Log('Stale cleanup: keeping the quarantine and the data folder');
    Exit;
  end;
  if Exec(ExpandConstant('{app}\StaleUninstaller.exe'), CleanupParams('--apply ' + Mode), '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode) then
    Log(Format('Stale cleanup --apply %s exit code: %d', [Mode, ResultCode]))
  else
    Log('Could not run the Stale cleanup: ' + SysErrorMessage(ResultCode));
end;
