# Task 7.2 — release cold start: process launch -> "input is live".
#
# The window logs one line per IME callback and one `ready` line when it opens with
# the editor focused (src/main.rs), and a `boot` line on the first instruction of
# `main`. Both carry an absolute wall clock in milliseconds (`ime.rs::stamp`), so a
# parent process can measure the interval without trusting its own poll loop:
#
#     process launch        = the timestamp taken immediately before Start-Process
#     boot                  = the `boot` line   -> the part spent before `main`
#     ready                 = the `ready` line  -> the part spent opening the window
#
# Run it from `spike/` after `cargo build --release`:
#
#     pwsh -File bench-cold-start.ps1
#     pwsh -File bench-cold-start.ps1 -Doc C:\path\to\big.md -Runs 3
#
# Median of the runs is the number task 7.2 asks for (<= 1 s). Cold vs warm is
# reported separately, because runs 2 and 3 reuse the file system cache for the
# document and the DLLs.

param(
    [string]$Doc = "sample.md",
    [int]$Runs = 3,
    [string]$Exe = ".\target\release\spike.exe"
)

$ErrorActionPreference = 'Stop'
$exe = (Resolve-Path $Exe).Path
$doc = (Resolve-Path $Doc).Path
$tmp = [System.IO.Path]::GetTempPath()
$docBytes = (Get-Item $doc).Length

Write-Output "exe: $exe"
Write-Output "doc: $doc ($docBytes bytes)"
Write-Output "runs: $Runs"
Write-Output ""

$rows = @()
function Read-Shared([string]$path) {
    # The child still holds the redirect open, so share the handle.
    try {
        $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    } catch {
        return $null
    }
    try {
        $sr = [System.IO.StreamReader]::new($fs)
        return $sr.ReadToEnd()
    } finally {
        $fs.Dispose()
    }
}

for ($run = 1; $run -le $Runs; $run++) {
    $err = Join-Path $tmp "typors-cold-$run.err"
    $out = Join-Path $tmp "typors-cold-$run.out"
    Remove-Item $err, $out -ErrorAction SilentlyContinue

    $started = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $proc = Start-Process -FilePath $exe -ArgumentList @($doc) `
        -RedirectStandardError $err -RedirectStandardOutput $out -PassThru -WindowStyle Normal

    $readyWall = $null
    $bootWall = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 5
        if (-not (Test-Path $err)) { continue }
        $text = Read-Shared $err
        if ($null -eq $text) { continue }
        if ($null -eq $bootWall) {
            $m = [regex]::Match($text, 'wall=(\d+)ms boot')
            if ($m.Success) { $bootWall = [int64]$m.Groups[1].Value }
        }
        $m = [regex]::Match($text, 'wall=(\d+)ms ready')
        if ($m.Success) { $readyWall = [int64]$m.Groups[1].Value; break }
        if ($proc.HasExited) { break }
    }

    $exited = $proc.HasExited
    if (-not $exited) { $proc.Kill(); $proc.WaitForExit() }

    if ($null -eq $readyWall) {
        Write-Output "run $run : FAILED - no 'ready' line (window did not open); see $err"
        $rows += $null
        continue
    }
    # `ready` is only written after the window exists and holds focus, so the
    # interval is what the user experiences as "I can type now".
    $totalMs = $readyWall - $started
    $preMainMs = if ($null -ne $bootWall) { $bootWall - $started } else { $null }
    $initMs = if ($null -ne $bootWall) { $readyWall - $bootWall } else { $null }
    $rows += [pscustomobject]@{
        Run = $run; TotalMs = $totalMs; PreMainMs = $preMainMs; InitMs = $initMs
    }
    Write-Output ("run {0} : start->ready {1,5} ms   (before main() {2,5} ms, main()+window {3,5} ms)" -f `
        $run, $totalMs, $preMainMs, $initMs)
}

$ok = $rows | Where-Object { $null -ne $_ }
if ($ok.Count -eq 0) {
    Write-Output ""
    Write-Output "result: FAIL - no run produced a 'ready' line"
    exit 1
}

$totals = $ok | ForEach-Object { $_.TotalMs } | Sort-Object
$median = $totals[[int][math]::Floor($totals.Count / 2)]
if ($totals.Count % 2 -eq 0) {
    $median = [int](($totals[$totals.Count / 2 - 1] + $totals[$totals.Count / 2]) / 2)
}

Write-Output ""
Write-Output ("totals (ms): {0}" -f ($totals -join ", "))
Write-Output ("median: {0} ms" -f $median)
if ($ok.Count -gt 1) {
    Write-Output ("cold (first run): {0} ms   warm runs: {1}" -f `
        $ok[0].TotalMs, (($ok | Select-Object -Skip 1 | ForEach-Object { $_.TotalMs }) -join ", "))
}
$verdict = if ($median -le 1000) { "PASS" } else { "FAIL" }
Write-Output ("result: {0} (median {1} ms vs the 1000 ms budget of task 7.2)" -f $verdict, $median)
if ($verdict -eq "PASS") { exit 0 } else { exit 1 }
