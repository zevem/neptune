; Native per-user installer. Signing can later use Inno Setup SignTool;
; release checksums/Ed25519 metadata already cover the final installer bytes.
#ifndef Version
  #error Version is required
#endif
#ifndef NumericVersion
  #error NumericVersion is required
#endif
[Setup]
AppId={{81244420-599A-4F10-BA79-F948D6DCABED}
AppName=Neptune
AppVersion={#Version}
VersionInfoVersion={#NumericVersion}
AppPublisher=Neptune
AppPublisherURL=https://neptune.rs
AppSupportURL=https://github.com/zevem/neptune/issues
DefaultDirName={localappdata}\Programs\Neptune
DefaultGroupName=Neptune
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=Neptune-{#Version}-windows-x64
SetupIconFile=..\assets\icons\neptune.ico
UninstallDisplayIcon={app}\neptune.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\x86_64-pc-windows-msvc\release\neptune.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\package\windows-browser\*"; DestDir: "{app}\browser"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\package\windows\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Neptune"; Filename: "{app}\neptune.exe"
Name: "{autodesktop}\Neptune"; Filename: "{app}\neptune.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\neptune.exe"; Description: "Launch Neptune"; Flags: nowait postinstall skipifsilent unchecked
