#ifndef MyAppVersion
  #define MyAppVersion "1.12.4"
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
SetupIconFile=..\..\assets\TxPlayer.ico
UninstallDisplayIcon={app}\txplayer.exe,0
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
Name: "{group}\TxPlayer"; Filename: "{app}\txplayer.exe"; IconFilename: "{app}\txplayer.exe"
Name: "{group}\{cm:UninstallProgram,TxPlayer}"; Filename: "{uninstallexe}"; IconFilename: "{app}\txplayer.exe"
Name: "{autodesktop}\TxPlayer"; Filename: "{app}\txplayer.exe"; IconFilename: "{app}\txplayer.exe"; Tasks: desktopicon

[Registry]
; Register ProgID + Windows Explorer / Total Commander "Open With" + Default Apps capabilities (HKCU, no admin needed)
Root: HKCU; Subkey: "Software\Classes\TxPlayer.AudioFile"; ValueType: string; ValueName: ""; ValueData: "TxPlayer Audio File"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\TxPlayer.AudioFile\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\txplayer.exe,0"
Root: HKCU; Subkey: "Software\Classes\TxPlayer.AudioFile\shell\open"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "TxPlayer"
Root: HKCU; Subkey: "Software\Classes\TxPlayer.AudioFile\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\txplayer.exe"" ""%1"""

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

; Register OpenWithProgids so TxPlayer appears in Windows Explorer & Total Commander right-click "Open With"
Root: HKCU; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.aiff\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.aif\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.wv\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.ape\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.dsf\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.dff\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.cue\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.iso\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.m3u\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.m3u8\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.pls\OpenWithProgids"; ValueType: string; ValueName: "TxPlayer.AudioFile"; ValueData: ""; Flags: uninsdeletevalue

; Register Windows Default Apps Capabilities
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "TxPlayer"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Native Rust audiophile music player & master library"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".flac"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".mp3"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".wav"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".aiff"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".aif"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m4a"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".aac"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ogg"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".opus"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".wv"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".ape"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".dsf"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".dff"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".cue"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".iso"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m3u"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".m3u8"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\TxPlayer\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pls"; ValueData: "TxPlayer.AudioFile"
Root: HKCU; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "TxPlayer"; ValueData: "Software\TxPlayer\Capabilities"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\txplayer.exe"; Description: "{cm:LaunchProgram,TxPlayer}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Remove self-contained install data folder, install directory, and any legacy AppData folders cleanly
Type: filesandordirs; Name: "{app}\data"
Type: filesandordirs; Name: "{app}"
Type: filesandordirs; Name: "{localappdata}\TxPlayer"
Type: filesandordirs; Name: "{userappdata}\TxPlayer"

[Code]
procedure CleanRegValuesByName(RootKey: Integer; const SubKey, Needle: String);
var
  Names: TArrayOfString;
  I: Integer;
begin
  if RegGetValueNames(RootKey, SubKey, Names) then
  begin
    for I := 0 to GetArrayLength(Names) - 1 do
    begin
      if Pos(Needle, Lowercase(Names[I])) > 0 then
        RegDeleteValue(RootKey, SubKey, Names[I]);
    end;
  end;
end;

procedure CleanRegValuesByData(RootKey: Integer; const SubKey, Needle: String);
var
  Names: TArrayOfString;
  I: Integer;
  ValStr: String;
begin
  if RegGetValueNames(RootKey, SubKey, Names) then
  begin
    for I := 0 to GetArrayLength(Names) - 1 do
    begin
      ValStr := '';
      if RegQueryMultiStringValue(RootKey, SubKey, Names[I], ValStr) or
         RegQueryStringValue(RootKey, SubKey, Names[I], ValStr) then
      begin
        if Pos(Needle, Lowercase(ValStr)) > 0 then
          RegDeleteValue(RootKey, SubKey, Names[I]);
      end;
    end;
  end;
end;

procedure CleanOpenWithListForExt(const Ext: String);
var
  SubKey, ValStr, MruStr: String;
  Names: TArrayOfString;
  I: Integer;
begin
  SubKey := 'Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\' + Ext + '\OpenWithList';
  if RegGetValueNames(HKCU, SubKey, Names) then
  begin
    for I := 0 to GetArrayLength(Names) - 1 do
    begin
      if Lowercase(Names[I]) <> 'mrulist' then
      begin
        ValStr := '';
        if RegQueryStringValue(HKCU, SubKey, Names[I], ValStr) then
        begin
          if Pos('txplayer.exe', Lowercase(ValStr)) > 0 then
          begin
            RegDeleteValue(HKCU, SubKey, Names[I]);
            if RegQueryStringValue(HKCU, SubKey, 'MRUList', MruStr) then
            begin
              StringChangeEx(MruStr, Names[I], '', True);
              RegWriteStringValue(HKCU, SubKey, 'MRUList', MruStr);
            end;
          end;
        end;
      end;
    end;
  end;
end;

procedure CleanPrefetchFiles;
var
  FindRec: TFindRec;
  PrefetchDir: String;
begin
  PrefetchDir := ExpandConstant('{win}\Prefetch');
  if FindFirst(PrefetchDir + '\TXPLAYER.EXE-*.pf', FindRec) then
  begin
    try
      repeat
        DeleteFile(PrefetchDir + '\' + FindRec.Name);
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Sweep Windows Explorer / Shell MRU & cache registry entries left behind by Windows itself
    CleanRegValuesByName(HKCU, 'Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache', 'txplayer');
    CleanRegValuesByName(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\FeatureUsage\AppSwitched', 'txplayer');
    CleanRegValuesByName(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\FeatureUsage\ShowJumpView', 'txplayer');
    CleanRegValuesByName(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\FeatureUsage\AppBadgeUpdated', 'txplayer');
    CleanRegValuesByName(HKCU, 'Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store', 'txplayer');
    CleanRegValuesByData(HKCU, 'Software\Microsoft\Windows\CurrentVersion\UFH\SHC', 'txplayer');

    CleanOpenWithListForExt('.flac');
    CleanOpenWithListForExt('.mp3');
    CleanOpenWithListForExt('.wav');
    CleanOpenWithListForExt('.aiff');
    CleanOpenWithListForExt('.aif');
    CleanOpenWithListForExt('.m4a');
    CleanOpenWithListForExt('.aac');
    CleanOpenWithListForExt('.ogg');
    CleanOpenWithListForExt('.opus');
    CleanOpenWithListForExt('.wv');
    CleanOpenWithListForExt('.ape');
    CleanOpenWithListForExt('.dsf');
    CleanOpenWithListForExt('.dff');
    CleanOpenWithListForExt('.cue');
    CleanOpenWithListForExt('.iso');
    CleanOpenWithListForExt('.m3u');
    CleanOpenWithListForExt('.m3u8');
    CleanOpenWithListForExt('.pls');

    // Remove Windows Prefetch trace when uninstalling with Administrator privileges
    CleanPrefetchFiles;
  end;
end;
