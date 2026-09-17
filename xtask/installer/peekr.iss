; Windows installer for peekr, built by `cargo xtask installer` (Inno Setup 6).
;
; The task passes the paths and the version in; nothing here is meant to be edited by hand:
;   ISCC /DVersion=0.1.0 /DStageDir=... /DOutputDir=... /DOutputName=... /DIconFile=... peekr.iss

#define AppName "Peekr"
#define Publisher "kvizyx"
#define Url "https://github.com/kvizyx/peekr"

[Setup]
AppId={{6F3B0E4A-2C55-4F35-9C0C-6C8F63A0F3D1}
AppName={#AppName}
AppVersion={#Version}
AppPublisher={#Publisher}
AppPublisherURL={#Url}
AppSupportURL={#Url}/issues
AppUpdatesURL={#Url}/releases
VersionInfoVersion={#Version}

; Per-user install, so it needs no administrator rights.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
UninstallDisplayName={#AppName} {#Version}
UninstallDisplayIcon={app}\peekr.exe

SetupIconFile={#IconFile}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}

; peekr sits in the tray, so it has to be closed before its files can be replaced.
CloseApplications=yes
CloseApplicationsFilter=peekr.exe

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startup"; Description: "Start {#AppName} when Windows starts"; GroupDescription: "Additional tasks:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional tasks:"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\peekr.exe"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\peekr.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
    ValueName: "{#AppName}"; ValueData: """{app}\peekr.exe"""; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\peekr.exe"; Description: "Run {#AppName} now"; Flags: nowait postinstall skipifsilent
