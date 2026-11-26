#ifndef MyAppVersion
  #define MyAppVersion "1.12.0"
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

[Run]
Filename: "{app}\txplayer.exe"; Description: "{cm:LaunchProgram,TxPlayer}"; Flags: nowait postinstall skipifsilent
