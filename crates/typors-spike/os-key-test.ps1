# Drive the real typors window through the OS keyboard queue.
#
# This is the closest the agent can get to a physical key press: the messages go
# through the same WndProc path gpui-pre-windows implements (WM_KEYDOWN ->
# handle_keydown_msg -> parse_normal_key -> PlatformInput::KeyDown -> keymap ->
# Newline action), and WM_CHAR is posted too, exactly as Windows does after
# TranslateMessage, so a text-channel second newline would show up.
param(
    [int] $Presses = 5,
    [int] $ExitDumpMs = 4000
)

$ErrorActionPreference = 'Stop'
Add-Type -Namespace Win32 -Name Api -MemberDefinition @'
[DllImport("user32.dll", SetLastError=true)]
public static extern bool PostMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
[DllImport("user32.dll")]
public static extern uint MapVirtualKey(uint uCode, uint uMapType);
[DllImport("user32.dll")]
public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")]
public static extern IntPtr GetForegroundWindow();
[DllImport("user32.dll")]
public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
[DllImport("user32.dll")]
public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
[DllImport("kernel32.dll")]
public static extern uint GetCurrentThreadId();
[DllImport("user32.dll")]
public static extern bool SetFocus(IntPtr hWnd);
'@

$spike = Join-Path $PSScriptRoot 'target\release\spike.exe'
$sample = Join-Path $PSScriptRoot 'sample.md'
$log = Join-Path $env:TEMP 'typors-os-keys.log'
$err = Join-Path $env:TEMP 'typors-os-keys.err'
Remove-Item $log, $err -ErrorAction SilentlyContinue

$env:TYPORS_EXIT_DUMP = "$ExitDumpMs"
$env:TYPORS_EDIT_LOG = '1'
$proc = Start-Process -FilePath $spike -ArgumentList $sample -RedirectStandardOutput $log -RedirectStandardError $err -PassThru
Start-Sleep -Milliseconds 1500

$hwnd = $proc.MainWindowHandle
if ($hwnd -eq [IntPtr]::Zero) {
    # MainWindowHandle can lag; poll briefly.
    for ($i = 0; $i -lt 20 -and $hwnd -eq [IntPtr]::Zero; $i++) {
        Start-Sleep -Milliseconds 150
        $proc.Refresh()
        $hwnd = $proc.MainWindowHandle
    }
}
if ($hwnd -eq [IntPtr]::Zero) {
    Write-Output 'RESULT: could not find the window handle'
    Stop-Process -Id $proc.Id -Force
    exit 2
}
Write-Output "hwnd=$hwnd pid=$($proc.Id)"

# Give the window keyboard focus so the posts land where a user's keys would.
$foregroundThread = [Win32.Api]::GetWindowThreadProcessId([Win32.Api]::GetForegroundWindow(), [ref]$null)
$targetThread = [Win32.Api]::GetWindowThreadProcessId($hwnd, [ref]$null)
[void][Win32.Api]::AttachThreadInput([Win32.Api]::GetCurrentThreadId(), $targetThread, $true)
[void][Win32.Api]::SetForegroundWindow($hwnd)
[void][Win32.Api]::SetFocus($hwnd)
[void][Win32.Api]::AttachThreadInput([Win32.Api]::GetCurrentThreadId(), $targetThread, $false)

$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$WM_CHAR = 0x0102
$VK_RETURN = 0x0D
$scan = [Win32.Api]::MapVirtualKey($VK_RETURN, 0)

# 1 keydown | scan<<16 | extended-less | previous-state-transition bit
$down = [IntPtr](1 -bor ([int]$scan -shl 16))
$up = [IntPtr]((1 -bor ([int]$scan -shl 16)) -bor (1 -shl 30) -bor (1 -shl 31))

for ($i = 1; $i -le $Presses; $i++) {
    [void][Win32.Api]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$VK_RETURN, $down)
    [void][Win32.Api]::PostMessage($hwnd, $WM_CHAR, [IntPtr]0x0D, $down)
    [void][Win32.Api]::PostMessage($hwnd, $WM_KEYUP, [IntPtr]$VK_RETURN, $up)
    Start-Sleep -Milliseconds 120
}
Write-Output "posted $Presses enter keydown/keyup pairs (plus WM_CHAR 0x0D)"

$proc.WaitForExit(15000) | Out-Null
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }

Write-Output '--- stdout ---'
Get-Content $log -ErrorAction SilentlyContinue
Write-Output '--- stderr (edit log) ---'
Get-Content $err -ErrorAction SilentlyContinue
