; Fotowache — Windows-Installer (Inno Setup 6).
;
; Installiert fuer den aktuellen Benutzer nach %LOCALAPPDATA%\Programs\Fotowache:
; keine Administratorrechte, kein UAC-Dialog, sauber deinstallierbar.
; Aufruf (scripts\paket-windows.ps1 erledigt das):
;   iscc /DVersion=2.0.0 /DQuelle=..\dist\Fotowache-windows installer\fotowache.iss

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Quelle
  #define Quelle "..\dist\Fotowache-windows"
#endif

[Setup]
AppId={{8A7A9D81-D5D6-49C4-96B6-AB6D9315304F}
AppName=Fotowache
AppVersion={#Version}
AppVerName=Fotowache {#Version}
AppPublisher=Fotowache
AppPublisherURL=https://github.com/robeertm/Fotowache
AppSupportURL=https://github.com/robeertm/Fotowache/issues
DefaultDirName={localappdata}\Programs\Fotowache
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Fotowache-{#Version}-windows-x64-setup
SetupIconFile=..\crates\app\assets\fotowache.ico
UninstallDisplayIcon={app}\fotowache.exe
UninstallDisplayName=Fotowache
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Quelle}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Fotowache"; Filename: "{app}\fotowache.exe"
Name: "{autodesktop}\Fotowache"; Filename: "{app}\fotowache.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\fotowache.exe"; Description: "{cm:LaunchProgram,Fotowache}"; Flags: nowait postinstall skipifsilent
; Nach einem Update aus Fotowache heraus (still, /STARTEN=1) wieder starten.
Filename: "{app}\fotowache.exe"; Flags: nowait; Check: NachUpdateStarten

[Code]
function NachUpdateStarten(): Boolean;
begin
  Result := ExpandConstant('{param:STARTEN|0}') = '1';
end;
