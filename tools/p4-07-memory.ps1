# The P4.07 driver: measure `E-MEM-1` (docs/30 §23.1) at the three lengths docs/31 P4.07 fixes,
# one process per length, and report whether the ceiling moved.
#
# Why a script and not just the test: `the_three_lengths_run_in_separate_processes` (in
# `crates/snapclip-capture/src/scroll/mem_probe.rs`) is the gate's check and it runs in whatever
# profile the gate uses. The number that goes into docs/30 is an L4 measurement, and docs/31 §2.2
# puts L4 in release. This driver is the release run, and it is the same shape as the P0.03 driver
# (`tools/p0-03-matching-cost.ps1`) for the same reason: `peak` is a process-global high-water mark,
# so a loop in one process would report the longest length's peak three times.
#
# What the run has to show (docs/31 P4.07 exit condition ①): the peak across 10,000 / 30,000 /
# 100,000 rows differs by at most 10%. What it also records, and what the "heap traffic cannot prove
# a drop in space" discipline (§22.6, quoting `benchmark-support/README.md`) requires, is the
# cumulative traffic and the spill file size — because a canvas that stopped allocating might have
# moved its rows to disk rather than stopped needing them.
#
# Usage:
#   pwsh -File tools/p4-07-memory.ps1
#   pwsh -File tools/p4-07-memory.ps1 -Lengths 10000,30000
#   pwsh -File tools/p4-07-memory.ps1 -Out docs/Temp/mem1.jsonl -Band 0.10
#
# Task and exit conditions: docs/31 §10 (P4.07). Results are consumed by docs/30 §22.6.
param(
    [string]$Out = '',
    [string[]]$Lengths = @(),
    [double]$Band = 0.10
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = "docs/Temp/mem1-$(Get-Date -Format yyyy-MM-dd).jsonl" }
$Out = Join-Path $root $Out
$logDir = Join-Path $root 'docs/Temp/mem1-logs'

# `pwsh -File script.ps1 -Lengths 10000,30000` hands the whole `10000,30000` string to one element,
# because `-File` does not re-parse arguments as PowerShell syntax (the same trap as P0.03/P0.04).
$Lengths = @($Lengths | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($Lengths.Count -eq 0) { $Lengths = @('10000', '30000', '100000') }

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $Out

Write-Host '[P4.07] building the probe (release)' -ForegroundColor Cyan
& cargo test --release -p snapclip-capture --lib --no-run 2>&1 |
    Select-Object -Last 3 | ForEach-Object { Write-Host "  $_" }
if ($LASTEXITCODE -ne 0) { throw "the probe did not build (exit $LASTEXITCODE)" }

# docs/31 exit condition ③ / docs/30 §23.5: the numbers are only comparable if the tree, the
# toolchain and the binary are recorded with them.
$binary = Get-ChildItem (Join-Path $root 'target/release/deps') -Filter 'snapclip_capture-*.exe' |
    Where-Object { $_.Name -notmatch '\.d\.exe$' } | Sort-Object LastWriteTime | Select-Object -Last 1
$header = [ordered]@{
    kind        = 'env'
    date        = (Get-Date).ToString('o')
    profile     = 'release'
    cross_len   = 1500
    extent      = 1080
    step_rows   = 120
    viewports   = 8
    budget      = 1500 * 1080 * 4 * 8
    lengths     = $Lengths
    band        = $Band
    rustc       = (rustc -V)
    cargo       = (cargo -V)
    os          = [System.Environment]::OSVersion.VersionString
    lockfile    = (Get-FileHash -Algorithm SHA256 (Join-Path $root 'Cargo.lock')).Hash
    binary      = if ($binary) { $binary.Name } else { 'unknown' }
    binary_hash = if ($binary) { (Get-FileHash -Algorithm SHA256 $binary.FullName).Hash } else { 'unknown' }
}
Add-Content -Path $Out -Value ($header | ConvertTo-Json -Compress)

# The children append their own lines: `mem1_measures_one_length` writes to `SNAPCLIP_MEM1_OUT` so
# that the hand-off does not go through a pipe (a captured pipe is the one thing a Windows sandbox
# refuses, and the release run has to work in the same places the gate does).
$env:SNAPCLIP_MEM1_OUT = $Out

$reports = @()
foreach ($length in $Lengths) {
    $log = Join-Path $logDir "$length.log"
    $env:SNAPCLIP_MEM1_LENGTH = $length
    $started = Get-Date
    & cargo test --release -p snapclip-capture --lib mem1_measures_one_length `
        -- --ignored --nocapture --test-threads=1 *> $log
    $exit = $LASTEXITCODE
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    if ($exit -ne 0) {
        Write-Host ("[P4.07] {0,7} rows FAILED ({1}s, exit {2}) — see {3}" -f $length, $elapsed, $exit, $log) -ForegroundColor Red
        Add-Content -Path $Out -Value (@{ kind = 'failure'; length = [int]$length; exit = $exit; log = $log } | ConvertTo-Json -Compress)
        continue
    }
    $row = Select-String -Path $Out -Pattern ('"length":' + $length + ',') | Select-Object -Last 1
    if (-not $row) {
        Write-Host ("[P4.07] {0,7} rows produced no result line ({1}s)" -f $length, $elapsed) -ForegroundColor Yellow
        continue
    }
    $report = $row.Line | ConvertFrom-Json
    $reports += $report
    Write-Host ("[P4.07] {0,7} rows  peak {1,10:N0} B  live {2,10:N0} B  traffic {3,12:N0} B  spill {4,12:N0} B  steps {5,5}  ({6}s)" -f `
        $report.rows, $report.peak, $report.live, $report.allocated, $report.spill_bytes, $report.steps, $elapsed)
}

Write-Host ''
Write-Host "[P4.07] measurements in $Out" -ForegroundColor Cyan

if ($reports.Count -lt 2) {
    Write-Host '  fewer than two lengths produced a report, so the ceiling cannot be compared' -ForegroundColor Yellow
    exit 1
}

$reports | ForEach-Object { $_ | Add-Member -NotePropertyName peak_over_baseline -NotePropertyValue ($_.peak - $_.live_before) -Force }
$reports | Format-Table rows, steps, @{ n = 'single_buffer_MiB'; e = { '{0:N1}' -f ($_.single_buffer / 1MB) } },
    @{ n = 'peak_MiB'; e = { '{0:N1}' -f ($_.peak / 1MB) } },
    @{ n = 'peak_over_baseline_MiB'; e = { '{0:N1}' -f ($_.peak_over_baseline / 1MB) } },
    @{ n = 'live_MiB'; e = { '{0:N1}' -f ($_.live / 1MB) } },
    @{ n = 'traffic_MiB'; e = { '{0:N1}' -f ($_.allocated / 1MB) } },
    @{ n = 'spill_MiB'; e = { '{0:N1}' -f ($_.spill_bytes / 1MB) } } -AutoSize

$low = ($reports | Measure-Object -Property peak_over_baseline -Minimum).Minimum
$high = ($reports | Measure-Object -Property peak_over_baseline -Maximum).Maximum
$spread = ($high - $low) / $low
$verdict = if ($spread -le $Band) { 'PASS' } else { 'FAIL' }
$color = if ($spread -le $Band) { 'Green' } else { 'Red' }
Write-Host ("[P4.07] ceiling over baseline {0:N0} B .. {1:N0} B, spread {2:P2} (allowed {3:P0}) — {4}" -f `
    $low, $high, $spread, $Band, $verdict) -ForegroundColor $color

$summary = [ordered]@{
    kind           = 'summary'
    lengths        = @($reports | ForEach-Object { $_.length })
    peak_low       = $low
    peak_high      = $high
    spread         = $spread
    band           = $Band
    verdict        = $verdict
    single_buffer  = @($reports | ForEach-Object { $_.single_buffer })
    spill_bytes    = @($reports | ForEach-Object { $_.spill_bytes })
}
Add-Content -Path $Out -Value ($summary | ConvertTo-Json -Compress)

if ($verdict -eq 'FAIL') { exit 1 }
