param(
    [string]$Version = "1.0.5",
    [string]$Makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
)
$ErrorActionPreference = "Stop"
$script = Join-Path $PSScriptRoot "typors.nsi"
$out = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Path $out -Force | Out-Null
if (-not (Test-Path $Makensis)) {
    throw "makensis.exe not found at '$Makensis'; pass -Makensis or install NSIS 3."
}
& $Makensis "/INPUTCHARSET" "UTF8" "/DVERSION=$Version" $script
if ($LASTEXITCODE -ne 0) { throw "makensis failed with $LASTEXITCODE" }
$installer = Join-Path $out "typors-$Version-setup.exe"
if (-not (Test-Path $installer)) { throw "installer not produced: $installer" }
Get-Item $installer | Select-Object FullName, Length, LastWriteTime
