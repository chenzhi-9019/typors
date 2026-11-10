; typors installer script (Inno Setup 6)
;
; Build via packaging\build-installer.ps1 rather than calling ISCC directly.
; All preprocessor defines below are overridable from the ISCC command line
; with /D<Name>=<value>; the defaults are the local-development values.
;
; This file contains non-ASCII text and MUST be saved as UTF-8 with BOM.

#define AppName "typors"
#define AppExeName "typors.exe"
#define AppPublisher "typors"

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#ifndef PayloadFile
  #define PayloadFile "..\target\release\typors.exe"
#endif

; VersionInfoVersion is a fixed numeric resource; allow a pre-release AppVersion
; (0.2.0-rc1) by passing the numeric prefix separately.
#ifndef VersionQuad
  #define VersionQuad AppVersion
#endif

[Setup]
; AppId is the identity of this application for Windows. It keys the uninstall
; registry entry and the Installed flag. NEVER change it: an installer with a
; different AppId is a different product, so it will install side by side and
; upgrades will not replace the previous version.
AppId={{f3c62e7d-9603-40b2-af12-ee14933a91a6}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#VersionQuad}
VersionInfoDescription={#AppName} Setup

; {autopf} resolves to "Program Files" for an all-users install and to
; "%LocalAppData%\Programs" for a current-user install, i.e. the correct
; per-mode landing directory in both cases.
DefaultDirName={autopf}\typors
DefaultGroupName={#AppName}
UsePreviousAppDir=yes

; Keep the directory page: the user must be able to choose the install dir.
DisableDirPage=no
DisableProgramGroupPage=yes

; Install per-user by default, but let the wizard (and the command line) offer
; the all-users mode. "dialog" adds the "Select Setup Install Mode" page at the
; start of the wizard; "commandline" keeps /ALLUSERS and /CURRENTUSER working.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline

; Payload is x64; refuse non-x64 machines and install into the 64-bit view.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Tell Explorer that file associations changed, so a .md association takes
; effect without a logoff.
ChangesAssociations=yes

; Two languages, but no "Select Language" page in the wizard: the first
; [Languages] entry (Chinese Simplified) is the default, and /LANG=english is
; still honoured. LanguageDetectionMethod=none keeps detection from silently
; overriding that default on an English-UI Windows.
ShowLanguageDialog=no
LanguageDetectionMethod=none

OutputDir=dist
OutputBaseFilename={#AppName}-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
AllowNoIcons=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppExeName}

; Signing is opt-in. When SIGNTOOL is not defined (the default) this directive
; is absent, so an unsigned build is produced and no sign tool need exist.
; Enable it with: ISCC /DSIGNTOOL=typors ... plus a registered sign tool.
#ifdef SIGNTOOL
SignTool={#SIGNTOOL}
#endif

[Languages]
; The first entry is the default language (ShowLanguageDialog=no).
Name: "chinesesimplified"; MessagesFile: "languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
english.MdAssocGroup=File association
english.MdAssocTask=Set typors as the default app for .md files
chinesesimplified.MdAssocGroup=文件关联
chinesesimplified.MdAssocTask=将 typors 设为 .md 文件的默认打开程序

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "mdassoc"; Description: "{cm:MdAssocTask}"; GroupDescription: "{cm:MdAssocGroup}"

[Files]
Source: "{#PayloadFile}"; DestDir: "{app}"; DestName: "{#AppExeName}"; Flags: ignoreversion

[Icons]
; {group} is used rather than {autoprograms}\... so that /NOICONS (and the
; "Don't create a Start Menu folder" choice) actually suppresses these entries.
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Comment: "Launch {#AppName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Comment: "Launch {#AppName}"; Tasks: desktopicon

[Registry]
; HKA follows the install mode: current-user -> HKCU\Software\Classes,
; all-users -> HKLM\Software\Classes. Everything here is gated on the "mdassoc"
; task, so unchecking it leaves the registry untouched.
;
; The whole typors.md ProgID belongs to us, so it is safe to delete the key.
Root: HKA; Subkey: "Software\Classes\typors.md"; ValueType: string; ValueName: ""; ValueData: "typors Markdown document"; Tasks: mdassoc; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\typors.md\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"; Tasks: mdassoc
Root: HKA; Subkey: "Software\Classes\typors.md\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""; Tasks: mdassoc
; Our own value name only: uninsdeletevalue cannot hit another app's entry.
Root: HKA; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "typors.md"; ValueData: ""; Tasks: mdassoc; Flags: uninsdeletevalue

; The .md default handler is deliberately NOT marked uninsdeletevalue: that flag
; would delete whatever handler is current at uninstall time, including one the
; user picked after installing typors. It is cleaned up in [Code] instead, and
; only while it still points at typors.md.
Root: HKA; Subkey: "Software\Classes\.md"; ValueType: string; ValueName: ""; ValueData: "typors.md"; Tasks: mdassoc

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  MdProgId = 'typors.md';

{ Mirrors the HKA root used by [Registry]: all-users writes HKLM\Software\Classes,
  current-user writes HKCU\Software\Classes. }
function ClassesRootKey: Integer;
begin
  if IsAdminInstallMode then
    Result := HKLM
  else
    Result := HKCU;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Current: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    { Clear the .md default only while it is still ours. If the user has since
      chosen another handler, leave their choice alone. The .md key itself is
      not deleted: it may carry other applications' data. }
    if RegQueryStringValue(ClassesRootKey, 'Software\Classes\.md', '', Current)
       and (CompareText(Current, MdProgId) = 0) then
      RegDeleteValue(ClassesRootKey, 'Software\Classes\.md', '');
  end;
end;
