#ifndef MyAppVersion
  #define MyAppVersion "1.12.2"
#endif

[Setup]
AppId={{8E9B7C54-4B8A-4D2C-9E3A-7F12D5C891A0}
AppName=TxPlayer
AppVersion={#MyAppVersion}
AppVerName=TxPlayer {#MyAppVersion}
AppPublisher=Tuxie
AppPublisherURL=https://tuxie.eu/txplayer/
AppSupportURL=https://github.com/tuxie80/txplayer/issues
AppUpdatesURL=https://github.com/tuxie80/txplayer/releases
DefaultDirName={autopf}\TxPlayer
DefaultGroupName=TxPlayer
AllowNoIcons=yes
LicenseFile=..\..\LICENSE
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir=..\..\target\windows
OutputBaseFilename=TxPlayer-v{#MyAppVersion}-windows-x86_64-setup
SetupIconFile=..\..\target\windows\TxPlayer.ico
UninstallDisplayIcon={app}\txplayer.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\release\txplayer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\TxPlayer"; Filename: "{app}\txplayer.exe"
Name: "{group}\{cm:UninstallProgram,TxPlayer}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\TxPlayer"; Filename: "{app}\txplayer.exe"; Tasks: desktopicon

[Registry]
; Register TxPlayer cleanly in Windows "Open With" with quoted "%1" argument passing
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "TxPlayer"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\txplayer.exe,0"
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\txplayer.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".flac"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".mp3"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".wav"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".aiff"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".aif"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m4a"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".aac"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ogg"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".opus"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".wv"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".ape"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".dsf"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".dff"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".cue"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".iso"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m3u"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".m3u8"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\Applications\txplayer.exe\SupportedTypes"; ValueType: string; ValueName: ".pls"; ValueData: ""

[Run]
Filename: "{app}\txplayer.exe"; Description: "{cm:LaunchProgram,TxPlayer}"; Flags: nowait postinstall skipifsilent
