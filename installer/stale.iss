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
// Before anything is removed, Stale empties or keeps its quarantine and removes
// %ProgramData%\Stale Uninstaller (StaleUninstaller.exe --uninstall-cleanup). Exit code 2 means
// the user chose "Cancel": the uninstall stops. A silent uninstall passes --silent, so nobody is
// asked and a quarantine is kept. If the exe cannot be started (e.g. it is already gone), the
// uninstall goes on.
function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
  Params: String;
begin
  Result := True;
  Params := '--uninstall-cleanup';
  if UninstallSilent then
    Params := Params + ' --silent';
  if Exec(ExpandConstant('{app}\StaleUninstaller.exe'), Params, '', SW_SHOWNORMAL, ewWaitUntilTerminated, ResultCode) then
  begin
    Log(Format('Stale cleanup exit code: %d', [ResultCode]));
    if ResultCode = 2 then
      Result := False;
  end
  else
    Log('Could not run the Stale cleanup: ' + SysErrorMessage(ResultCode));
end;
