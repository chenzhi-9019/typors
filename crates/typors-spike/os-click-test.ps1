# Same OS-level driving, but the way a user actually works: click into the
# document first, then press Enter repeatedly (optionally holding it, i.e.
# repeated WM_KEYDOWN with the "previous state" bit, no WM_KEYUP in between).
param(
    [int] $Presses = 5,
    [int] $ExitDumpMs = 5000,
    [int] $ClickX = 200,
    [int] $ClickY = 120,
    [switch] $Hold
)

$ErrorActionPreference = 'Stop'
Add-Type -Namespace Win32 -Name Api2 -MemberDefinition @'
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
[DllImport("user32.dll")]
public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
[StructLayout(LayoutKind.Sequential)]
public struct POINT { public int x; public int y; }
'@

$spike = Join-Path $PSScriptRoot 'target\release\spike.exe'
$sample = Join-Path $PSScriptRoot 'sample.md'
$log = Join-Path $env:TEMP 'typors-os-click.log'
$err = Join-Path $env:TEMP 'typors-os-click.err'
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

$targetThread = [Win32.Api2]::GetWindowThreadProcessId($hwnd, [ref]$null)
[void][Win32.Api2]::AttachThreadInput([Win32.Api2]::GetCurrentThreadId(), $targetThread, $true)
[void][Win32.Api2]::SetForegroundWindow($hwnd)
[void][Win32.Api2]::AttachThreadInput([Win32.Api2]::GetCurrentThreadId(), $targetThread, $false)

$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP = 0x0202
$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$WM_CHAR = 0x0102
$VK_RETURN = 0x0D
$scan = [Win32.Api2]::MapVirtualKey($VK_RETURN, 0)

# The click goes in *client* coordinates of the window (gpui reads
# `MouseDownEvent.position` in window coordinates, which for a maximised-less
# default window equals client coordinates).
$pt = New-Object Win32.Api2+POINT
$pt.x = $ClickX; $pt.y = $ClickY
$clickLparam = [IntPtr](($ClickY -shl 16) -bor $ClickX)
[void][Win32.Api2]::PostMessage($hwnd, $WM_LBUTTONDOWN, [IntPtr]1, $clickLparam)
Start-Sleep -Milliseconds 80
[void][Win32.Api2]::PostMessage($hwnd, $WM_LBUTTONUP, [IntPtr]0, $clickLparam)
Start-Sleep -Milliseconds 200
Write-Output "clicked at client ($ClickX,$ClickY)"

$down = [IntPtr](1 -bor ([int]$scan -shl 16))
$held = [IntPtr]((1 -bor ([int]$scan -shl 16)) -bor (1 -shl 30))
$up = [IntPtr]((1 -bor ([int]$scan -shl 16)) -bor (1 -shl 30) -bor (1 -shl 31))

for ($i = 1; $i -le $Presses; $i++) {
    $lparam = if ($Hold) { $held } else { $down }
    [void][Win32.Api2]::PostMessage($hwnd, $WM_KEYDOWN, [IntPtr]$VK_RETURN, $lparam)
    [void][Win32.Api2]::PostMessage($hwnd, $WM_CHAR, [IntPtr]0x0D, $lparam)
    if (-not $Hold) { [void][Win32.Api2]::PostMessage($hwnd, $WM_KEYUP, [IntPtr]$VK_RETURN, $up) }
    Start-Sleep -Milliseconds 120
}
Write-Output ("posted {0} enters{1}" -f $Presses, $(if ($Hold) { ' held (bit30, no keyup)' } else { '' }))

$proc.WaitForExit(15000) | Out-Null
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }

Write-Output '--- stdout ---'
$out = Get-Content $log -ErrorAction SilentlyContinue
# only the tail of the buffer dump matters; the text is long
$out | ForEach-Object { if ($_.Length -gt 160) { $_.Substring(0, 160) + '...' } else { $_ } }
Write-Output '--- edit log ---'
Get-Content $err -ErrorAction SilentlyContinue | Select-String -Pattern 'edit |dump:' | ForEach-Object { $_.Line }
