; RSClipping installer - dark theme (Inno Setup 6.4+ for DarkMode=force)
; Build:
;   ISCC.exe /DMyAppVersion=0.1.0 packaging\rsclipping.iss
; In CI, install Inno Setup 6 (choco install innosetup) then run the above.

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#define MyAppName "RSClipping"
#define MyAppExeName "rsclipping.exe"
#define MyAppPublisher "RSClipping Project"
#define MyAppURL "https://github.com/human757-fin/rsclipping"

[Setup]
AppId={{9B9D4F2C-6A3E-48D1-A0B8-6E17C3BB5521}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\release
OutputBaseFilename=rsclipping-setup-{#MyAppVersion}
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
WizardResizable=no
DarkMode=force
SetupIconFile=assets\rsclipping.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}
ArchitecturesInstallIn64BitMode=x64compatible
SetupLogging=yes
DisableWelcomePage=no
RestartApplications=no
CloseApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"; Flags: unchecked
Name: "startup"; Description: "Start {#MyAppName} daemon at &logon (global hotkeys)"; GroupDescription: "Additional options:";

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "assets\rsclipping.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "portable-README.txt"; DestDir: "{app}"; DestName: "README.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\rsclipping.ico"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\rsclipping.ico"; Tasks: desktopicon

[Registry]
; Start with Windows using the installed binary (Launch at logon).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "RSClipping Daemon"; \
  ValueData: """{app}\{#MyAppExeName}"" daemon"; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\{#MyAppExeName}"; Parameters: "gui"; Description: "Launch {#MyAppName} now"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}"

[Code]
const
  DWMWA_USE_IMMERSIVE_DARK_MODE = 20;

type
  THandle = NativeInt;

procedure DwmSetWindowAttribute(hwnd: HWND; attr: DWORD; const ref: Boolean; cbSize: DWORD);
  external 'DwmSetWindowAttribute@dwmapi.dll stdcall';

// Dark-titlebar the wizard window on Windows 10 1809+.
procedure SetDarkTitleBar(h: HWND);
var
  enabled: Boolean;
begin
  enabled := True;
  try
    DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, enabled, SizeOf(enabled));
  except
  end;
end;

procedure InitializeWizard;
begin
  SetDarkTitleBar(WizardForm.Handle);
  WizardForm.Color := clNone; // DarkMode=force handles the background
  WizardForm.WelcomeLabel1.Font.Color := clWhite;
  WizardForm.WelcomeLabel2.Font.Color := clSilver;
  WizardForm.PageNameLabel.Font.Color := clWhite;
  WizardForm.PageDescriptionLabel.Font.Color := clSilver;
  SetDarkTitleBar(WizardForm.Handle);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssDone then
  begin
    SetDarkTitleBar(WizardForm.Handle);
  end;
end;