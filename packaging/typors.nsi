; typors installer (NSIS 3 / MUI2).
;
; Every optional feature is an unchecked section: the user must choose file
; association, desktop shortcut and Start-menu shortcut. The core files are the
; only required section. The Finish page's "run typors" box is also unchecked.
;
; Build:
;   makensis /DVERSION=0.1.0 packaging\typors.nsi

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !define VERSION "0.1.0"
!endif

Name "typors"
OutFile "dist\typors-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\typors"
InstallDirRegKey HKCU "Software\typors" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "typors - hybrid Markdown editor"

!define MUI_ICON "..\assets\typors.ico"
!define MUI_UNICON "..\assets\typors.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\typors.exe"
!define MUI_FINISHPAGE_RUN_TEXT "立即运行 typors"
!define MUI_FINISHPAGE_RUN_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

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
  WriteRegStr HKCU "Software\typors" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "DisplayName" "typors"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "DisplayIcon" "$INSTDIR\assets\typors.ico"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors" "NoRepair" 1
SectionEnd

Section /o "关联 Markdown (.md) 文件" SecAssoc
  ExecWait '"$INSTDIR\typors.exe" --register'
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
  ExecWait '"$INSTDIR\typors.exe" --unregister'
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
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\typors"
  DeleteRegKey HKCU "Software\typors"
SectionEnd
