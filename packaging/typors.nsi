; typors installer (NSIS 3 / MUI2).
;
; The first choice is the installation scope: every user (Program Files + HKLM)
; or only the current user (%LOCALAPPDATA% + HKCU). It has no default: the user
; must click one of the two radio buttons, and Next is disabled until then.
;
; Every optional feature is an unchecked section: file association, desktop
; shortcut and Start-menu shortcut are all opt-in. The Finish page's "run
; typors" box is unchecked too.
;
; Build:
;   makensis /DVER=1.0.0 packaging\typors.nsi

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "1.0.1"
!endif

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors"

Name "typors"
OutFile "dist\typors-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\typors"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "typors - hybrid Markdown editor"

Var InstallScope
Var ScopeAllRadio
Var ScopeMeRadio
Var Elevated

!define MUI_ICON "..\assets\typors.ico"
!define MUI_UNICON "..\assets\typors.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\typors.exe"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 typors"
!define MUI_FINISHPAGE_RUN_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
Page custom ScopePageCreate ScopePageLeave
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

!macro WriteScopeRegStr SUBKEY NAME VALUE
  ${If} $InstallScope == "all"
    WriteRegStr HKLM "${SUBKEY}" "${NAME}" "${VALUE}"
  ${Else}
    WriteRegStr HKCU "${SUBKEY}" "${NAME}" "${VALUE}"
  ${EndIf}
!macroend

!macro WriteScopeRegDWORD SUBKEY NAME VALUE
  ${If} $InstallScope == "all"
    WriteRegDWORD HKLM "${SUBKEY}" "${NAME}" "${VALUE}"
  ${Else}
    WriteRegDWORD HKCU "${SUBKEY}" "${NAME}" "${VALUE}"
  ${EndIf}
!macroend

!macro DeleteScopeRegKey SUBKEY
  ${If} $InstallScope == "all"
    DeleteRegKey HKLM "${SUBKEY}"
  ${Else}
    DeleteRegKey HKCU "${SUBKEY}"
  ${EndIf}
!macroend

Function SetScopePaths
  ${If} $InstallScope == "all"
    SetShellVarContext all
    ${If} $INSTDIR == "$LOCALAPPDATA\Programs\typors"
      StrCpy $INSTDIR "$PROGRAMFILES64\typors"
    ${EndIf}
  ${Else}
    SetShellVarContext current
    ${If} $INSTDIR == "$PROGRAMFILES64\typors"
      StrCpy $INSTDIR "$LOCALAPPDATA\Programs\typors"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function ScopePageCreate
  ; The all-users path relaunches this installer elevated with /ELEVATED=1;
  ; that instance has already made the choice and skips this page.
  ${If} $Elevated == "1"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "选择安装范围" "请选择为所有用户安装，还是仅为当前用户安装。"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateRadioButton} 0 0 100% 14u "为所有人安装（Program Files，需要管理员权限）"
  Pop $ScopeAllRadio
  ${NSD_CreateRadioButton} 0 22u 100% 14u "仅为我安装（当前用户，写入 %LOCALAPPDATA%）"
  Pop $ScopeMeRadio
  ${NSD_OnClick} $ScopeAllRadio ScopeRadioClicked
  ${NSD_OnClick} $ScopeMeRadio ScopeRadioClicked

  ; No default: Next stays disabled until the user picks a scope.
  GetDlgItem $0 $HWNDPARENT 1
  EnableWindow $0 0
  nsDialogs::Show
FunctionEnd

Function ScopeRadioClicked
  Pop $0
  ${If} $0 == $ScopeAllRadio
    StrCpy $InstallScope "all"
  ${Else}
    StrCpy $InstallScope "me"
  ${EndIf}
  GetDlgItem $0 $HWNDPARENT 1
  EnableWindow $0 1
FunctionEnd

Function ScopePageLeave
  ${If} $InstallScope == ""
    MessageBox MB_ICONEXCLAMATION|MB_OK "请选择“为所有人安装”或“仅为我安装”。"
    Abort
  ${EndIf}
  ${If} $InstallScope == "all"
    ${If} $Elevated != "1"
      ExecShell "runas" "$\"$EXEPATH$\"" "/ELEVATED=1 /SCOPE=all"
      Quit
    ${EndIf}
  ${EndIf}
  Call SetScopePaths
FunctionEnd

Function .onInit
  ClearErrors
  ${GetOptions} $CMDLINE "/ELEVATED=" $0
  ${If} $0 == "1"
    StrCpy $Elevated "1"
    StrCpy $InstallScope "all"
  ${EndIf}

  ; Silent installs cannot show the scope page; require /SCOPE=all or /SCOPE=me.
  ${GetOptions} $CMDLINE "/SCOPE=" $0
  ${If} $0 == "all"
    StrCpy $InstallScope "all"
  ${ElseIf} $0 == "me"
    StrCpy $InstallScope "me"
  ${EndIf}
  ${If} ${Silent}
    ${If} $InstallScope == ""
      SetErrorLevel 2
      Quit
    ${EndIf}
    ${If} $InstallScope == "all"
      ${If} $Elevated != "1"
        ExecShell "runas" "$\"$EXEPATH$\"" "/ELEVATED=1 /SCOPE=all /S"
        Quit
      ${EndIf}
    ${EndIf}
    Call SetScopePaths
  ${EndIf}
FunctionEnd

Function un.onInit
  ; An all-users uninstall needs elevation to remove Program Files/HKLM entries.
  ${GetOptions} $CMDLINE "/ELEVATED=" $0
  ${If} $0 != "1"
    ReadRegStr $1 HKLM "Software\typors" "InstallDir"
    ${If} $1 == "$INSTDIR"
      ExecShell "runas" "$\"$INSTDIR\uninstall.exe$\"" "/ELEVATED=1"
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "typors 核心文件（必需）" SecCore
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\target\release\typors.exe"
  File "..\target\release\typors_runtime.dll"
  File "..\README.md"
  File "..\sample.md"
  SetOutPath "$INSTDIR\assets"
  File /r "..\assets\*.*"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  !insertmacro WriteScopeRegStr "Software\typors" "InstallDir" "$INSTDIR"
  !insertmacro WriteScopeRegStr "Software\typors" "InstallScope" "$InstallScope"
  !insertmacro WriteScopeRegStr "${UNINST_KEY}" "DisplayName" "typors"
  !insertmacro WriteScopeRegStr "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  !insertmacro WriteScopeRegStr "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\assets\typors.ico"
  !insertmacro WriteScopeRegStr "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  !insertmacro WriteScopeRegDWORD "${UNINST_KEY}" "NoModify" 1
  !insertmacro WriteScopeRegDWORD "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section /o "关联 Markdown (.md) 文件" SecAssoc
  ${If} $InstallScope == "all"
    ExecWait '"$INSTDIR\typors.exe" --register-all'
  ${Else}
    ExecWait '"$INSTDIR\typors.exe" --register'
  ${EndIf}
SectionEnd

Section /o "创建桌面快捷方式" SecDesktop
  CreateShortcut "$DESKTOP\typors.lnk" "$INSTDIR\typors.exe" "" "$INSTDIR\assets\typors.ico" 0
SectionEnd

Section /o "创建开始菜单快捷方式" SecStartMenu
  CreateDirectory "$SMPROGRAMS\typors"
  CreateShortcut "$SMPROGRAMS\typors\typors.lnk" "$INSTDIR\typors.exe" "" "$INSTDIR\assets\typors.ico" 0
  CreateShortcut "$SMPROGRAMS\typors\卸载 typors.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  ; The uninstaller runs elevated; detect the scope from the machine key.
  ReadRegStr $0 HKLM "Software\typors" "InstallDir"
  ${If} $0 == "$INSTDIR"
    StrCpy $InstallScope "all"
    SetShellVarContext all
    ExecWait '"$INSTDIR\typors.exe" --unregister-all'
  ${Else}
    StrCpy $InstallScope "me"
    SetShellVarContext current
    ExecWait '"$INSTDIR\typors.exe" --unregister'
  ${EndIf}

  Delete "$INSTDIR\typors.exe"
  Delete "$INSTDIR\typors_runtime.dll"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\sample.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\assets"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\typors.lnk"
  Delete "$SMPROGRAMS\typors\typors.lnk"
  Delete "$SMPROGRAMS\typors\卸载 typors.lnk"
  RMDir "$SMPROGRAMS\typors"
  !insertmacro DeleteScopeRegKey "${UNINST_KEY}"
  !insertmacro DeleteScopeRegKey "Software\typors"
SectionEnd
