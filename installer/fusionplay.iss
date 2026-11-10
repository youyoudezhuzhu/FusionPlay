; FusionPlay Windows installer (Inno Setup 6).
;
; Build (from the repository root, after flutter/tool/build_windows.ps1 or the
; equivalent CI steps have produced the release folder):
;
;   ISCC.exe /DMyAppVersion=1.3.0.0 /DMyAppDisplayVersion=1.3.0 ^
;            /Odist installer\fusionplay.iss
;
; - /DMyAppVersion: 4-part numeric version used by AppVersion/VersionInfoVersion
;   and the uninstall entry (defaults to 1.3.0.0).
; - /DMyAppDisplayVersion: user-visible version used in the output filename and
;   AppVerName (defaults to 1.3.0).
; - /O<dir>: redirect the output into dist/ (OutputDir below is the default for
;   manual runs and lands in installer/output/).
;
; All [Files] Source paths are relative to this script's directory, i.e.
; installer\..\flutter\build\windows\x64\runner\Release. The release folder
; must already contain FusionPlay.exe, FusionPlayCore.exe, data\, the Flutter
; DLLs and the plugin DLLs. See installer/README.md.
;
; The installer needs administrator rights: it writes to Program Files and
; creates an inbound Windows Firewall rule for the sidecar (so AirPlay, DLNA and
; MiPlay traffic is not blocked). The rule is removed by the uninstaller.

#define MyAppName "FusionPlay"
#define MyAppPublisher "FusionPlay Community"
#define MyAppURL "https://github.com/youyoudezhuzhu/FusionPlay"
#ifndef MyAppVersion
  #define MyAppVersion "1.3.0.0"
#endif
#ifndef MyAppDisplayVersion
  #define MyAppDisplayVersion "1.3.0"
#endif

[Setup]
; AppId must stay stable across versions so upgrades replace the application
; instead of installing a second copy.
AppId={{8F2C9A74-3D61-4E0B-9C5F-6A7B8D9E0F12}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppDisplayVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription={#MyAppName} Setup
VersionInfoProductName={#MyAppName}
VersionInfoVersion={#MyAppVersion}
VersionInfoCopyright=Copyright (C) 2026 FusionPlay contributors. Licensed under AGPL-3.0.
DefaultDirName={autopf}\FusionPlay
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
AllowNoIcons=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\LICENSE
SetupIconFile=..\flutter\windows\runner\resources\app_icon.ico
UninstallDisplayIcon={app}\FusionPlay.exe
UninstallDisplayName={#MyAppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
OutputDir=output
OutputBaseFilename=FusionPlay-Setup-{#MyAppDisplayVersion}-x64

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Everything Flutter produced for the Release bundle, recursively: FusionPlay.exe,
; FusionPlayCore.exe (staged next to it by build_windows.ps1 / CI), data\,
; flutter_windows.dll and plugin DLLs.
Source: "..\flutter\build\windows\x64\runner\Release\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\FusionPlay.exe"; WorkingDir: "{app}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\FusionPlay.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
; Best-effort cleanup of a stale rule from a previous install, then allow the
; sidecar through the Windows Firewall on private/domain networks. The program
; based rule covers AirPlay (RTSP/RTP), DLNA (SSDP/HTTP) and MiPlay traffic.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""FusionPlay"""; Flags: runhidden ignoreerrors; StatusMsg: "Removing stale FusionPlay firewall rules..."
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""FusionPlay"" dir=in action=allow program=""{app}\FusionPlayCore.exe"" enable=yes profile=private,domain"; Flags: runhidden; StatusMsg: "Allowing FusionPlay through Windows Firewall..."
Filename: "{app}\FusionPlay.exe"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Remove the firewall rule created above. Runs only when the uninstaller for the
; installed version runs (an upgrade removes the old rule before the new [Run]
; section re-adds it).
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""FusionPlay"""; Flags: runhidden ignoreerrors

[Code]
// User data (%LOCALAPPDATA%\FusionPlay: settings.json, logs, artwork cache,
// MiPlay identity) is intentionally kept by default so upgrades and reinstalls
// preserve the receiver configuration. Ask during interactive uninstall;
// silent uninstalls always keep the data.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  DataDir := ExpandConstant('{localappdata}\FusionPlay');
  if (not DirExists(DataDir)) or UninstallSilent then
    Exit;
  if MsgBox('Also delete FusionPlay user data?' + #13#10 + #13#10 + DataDir + #13#10 + #13#10 +
            'Choose No to keep settings, logs and artwork caches for a future reinstall.',
            mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    DelTree(DataDir, True, True, True);
end;
