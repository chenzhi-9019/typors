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
assets\typors.ico
assets\sample-picture.png
sample.md
README.md
```

## Build

```powershell
pwsh packaging\build-nsis.ps1
# or: pwsh packaging\build-nsis.ps1 -Version 0.2.0
# -> packaging\dist\typors-1.0.0-setup.exe
```

The script passes `/INPUTCHARSET UTF8` to `makensis` because the script and the
UI strings are UTF-8 Chinese.

## User choices

The installer intentionally does **not** preselect optional behaviour:

| Page / section | Default |
| --- | --- |
| Install directory | `%LOCALAPPDATA%\Programs\typors`, editable |
| typors core files | Required |
| `.md` association | **Unchecked**; user must tick it |
| Desktop shortcut | **Unchecked** |
| Start-menu shortcut | **Unchecked** |
| Finish page "run typors" | **Unchecked** |

The association section runs `typors.exe --register`; the uninstaller runs
`--unregister`. On Windows 10/11 the first-run prompt also opens *Default Apps*
because only the user can confirm the `UserChoice` hash.

## Silent smoke test (used on this machine)

```powershell
$dir = "$env:TEMP\typors-install-test"
.\packaging\dist\typors-1.0.0-setup.exe /S /D=$dir
& "$dir\typors.exe" --dump-media "$dir\sample.md"
& "$dir\uninstall.exe" /S
```

A silent install selects only the required core section, so it does not touch
file associations or shortcuts.

## Uninstall

The uninstaller removes the files, shortcuts and the per-user
`HKCU\Software\typors` / `HKCU\Software\Classes\typors.md` registrations.
