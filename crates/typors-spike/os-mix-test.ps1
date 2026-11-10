# The user's own sequence: click, Enter, type a character, Enter, type, Enter ...
# A character arrives as WM_CHAR -> InputHandler::replace_text_in_range, exactly
# like a real keystroke. If interleaving the two channels is what breaks Enter,
# this reproduces it.
param(
    [int] $Cycles = 4,
    [int] $ExitDumpMs = 6000,
    [int] $ClickX = 200,
    [int] $ClickY = 120
)

$ErrorActionPreference = 'Stop'
Add-Type -Namespace Win32 -Name Api3 -MemberDefinition @'
[DllImport("user32.dll", SetLastError=true)]
public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll")]
public static extern uint MapVirtualKey(uint uCode, uint uMapType);
[DllImport("user32.dll")]
public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")]
public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
[DllImport("kernel32.dll")]
public static extern uint GetCurrentThreadId();
'@

$spike = Join-Path $PSScriptRoot 'target\release\spike.exe'
$sample = Join-Path $PSScriptRoot 'sample.md'
$log = Join-Path $env:TEMP 'typors-os-mix.log'
$err = Join-Path $env:TEMP 'typors-os-mix.err'
Remove-Item $log, $err -ErrorAction SilentlyContinue

$env:TYPORS_EXIT_DUMP = "$ExitDumpMs"
$env:TYPORS_EDIT_LOG = '1'
$proc = Start-Process -FilePath $spike -ArgumentList $sample -RedirectStandardOutput $log -RedirectStandardError $err -PassThru
Start-Sleep -Milliseconds 1500

$hwnd = $proc.MainWindowHandle
for ($i = 0; $i -lt 20 -and $hwnd -eq [IntPtr]::Zero; $i++) {
    Start-Sleep -Milliseconds 150
    $proc.Refresh()
    $hwnd = $proc.MainWindowHandle
}
if ($hwnd -eq [IntPtr]::Zero) { Write-Output 'RESULT: no window handle'; Stop-Process -Id $proc.Id -Force; exit 2 }

$targetThread = [Win32.Api3]::GetWindowThreadProcessId($hwnd, [ref]$null)
[void][Win32.Api3]::AttachThreadInput([Win32.Api3]::GetCurrentThreadId(), $targetThread, $true)
[void][Win32.Api3]::SetForegroundWindow($hwnd)
[void][Win32.Api3]::AttachThreadInput([Win32.Api3]::GetCurrentThreadId(), $targetThread, $false)

$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP = 0x0202
$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$WM_CHAR = 0x0102
$VK_RETURN = 0x0D
$VK_A = 0x41
$scanReturn = [Win32.Api3]::MapVirtualKey($VK_RETURN, 0)
$scanA = [Win32.Api3]::MapVirtualKey($VK_A, 0)

$clickLparam = [IntPtr](($ClickY -shl 16) -bor $ClickX)
[void][Win32.Api3]::PostMessage($hwnd, $WM_LBUTTONDOWN, [IntPtr]1, $clickLparam)
Start-Sleep -Milliseconds 80
[void][Win32.Api3]::PostMessage($hwnd, $WM_LBUTTONUP, [IntPtr]0, $clickLparam)
Start-Sleep -Milliseconds 200
Write-Output "clicked at client ($ClickX,$ClickY)"

function Post-Key([IntPtr] $vk, [int] $scan, [int] $charCode) {
    $down = [IntPtr](1 -bor ($scan -shl 16))
    $up = [IntPtr]((1 -bor ($scan -shl 16)) -bor (1 -shl 30) -bor (1 -shl 31))
    [void][Win32.Api3]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$vk, $down)
    if ($charCode -ge 0) { [void][Win32.Api3]::PostMessage($hwnd, $WM_CHAR, [IntPtr]$charCode, $down) }
    [void][Win32.Api3]::PostMessage($hwnd, $WM_KEYUP, [IntPtr]$vk, $up)
}

for ($i = 1; $i -le $Cycles; $i++) {
    Post-Key $VK_RETURN $scanReturn 0x0D
    Start-Sleep -Milliseconds 150
    Post-Key $VK_A $scanA 0x78   # 'x'
    Start-Sleep -Milliseconds 150
}
Write-Output "posted $Cycles (enter, 'x') cycles"

$proc.WaitForExit(20000) | Out-Null
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }

Write-Output '--- stdout ---'
Get-Content $log -ErrorAction SilentlyContinue | ForEach-Object { if ($_.Length -gt 120) { $_.Substring(0, 120) + '...' } else { $_ } }
Write-Output '--- edits ---'
Get-Content $err -ErrorAction SilentlyContinue | Select-String -SimpleMatch 'edit ','dump:' | ForEach-Object { $_.Line }
