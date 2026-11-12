# typors Windows installer (NSIS)

The supported installer is `packaging/typors.nsi`, built with NSIS 3.
The old Inno Setup files (`typors.iss`, `build-installer.ps1`,
`languages/`) are history and are not used.

## Prerequisites

- NSIS 3 installed, normally at
  `C:\Program Files (x86)\NSIS\makensis.exe` (the build script can take
  `-Makensis`).
- Release payload built first:

```powershell
cargo build --release --workspace
```

That produces:

```text
target\release\typors.exe          ~145 KB loader
target\release\typors_runtime.dll  ~16 MB runtime
target\release\typors_mermaid.dll  ~28 MB Mermaid (lazy)
assets\typors.ico
assets\sample-picture.png
sample.md
README.md
```

## Build

```powershell
pwsh packaging\build-nsis.ps1
# or: pwsh packaging\build-nsis.ps1 -Version 0.2.0
# -> packaging\dist\typors-1.0.4-setup.exe
```

The script passes `/INPUTCHARSET UTF8` to `makensis` because the script and the
UI strings are UTF-8 Chinese.

## User choices

The installer intentionally does **not** preselect optional behaviour:

| Page / section | Default |
| --- | --- |
| Install scope (all users / just me) | **No default**; Next disabled until chosen |
| Install directory | all users: `%PROGRAMFILES%\typors`; just me: `%LOCALAPPDATA%\Programs\typors`, editable |
| typors core files | Required |
| `.md` association | **Unchecked**; user must tick it |
| Desktop shortcut | **Unchecked** |
| Start-menu shortcut | **Unchecked** |
| Finish page "run typors" | **Unchecked** |

All-users installation writes `HKLM\Software\typors` and registers the
association under HKLM (`--register-all`); just-me writes HKCU (`--register`).
The all-users path relaunches the installer elevated via `ExecShell "runas"`.
Uninstall detects the scope from the machine key and elevates for all-users.

## Silent smoke test (used on this machine)

```powershell
$dir = "$env:TEMP\typors-install-test"
.\packaging\dist\typors-1.0.4-setup.exe /S /SCOPE=me /D=$dir
& "$dir\typors.exe" --dump-media "$dir\sample.md"
& "$dir\uninstall.exe" /S
```

Per-user silent installs do not prompt for UAC. A silent all-users install
(`/SCOPE=all`, no `/D`) relaunches elevated and therefore needs UAC approval; it
cannot be tested unattended.

## Uninstall

The uninstaller removes the files, shortcuts and the per-user
`HKCU\Software\typors` / `HKCU\Software\Classes\typors.md` registrations.
