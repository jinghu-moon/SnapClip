# The P5.06 driver: measure `E-PERF-4` (docs/30 §23.1) and the two latencies §23.3 still owed.
#
# Why a script and not just the tests: two of the three halves are L4 measurements and docs/31 §2.2
# puts L4 in release. The cost probe (`latency_probe::perf4_measures_the_costs`) writes one line per
# (kind, cross_len) so that a reader can subtract the box filter from the whole window refresh; the
# driver case (`loop_control::tests::latency_measures_one_run`) writes one line per run because a
# `Scroll Response` distribution is a property of the run, not of a nanosecond; the consumer case
# (`windows::win::d2d::tests::the_overlay_thread_never_blocks_longer_than_eight_ms`) prints its four
# rates because it needs a D3D11 device and cannot be driven by an environment variable at all.
#
# What the run has to show (docs/31 P5.06 exit conditions ② and ③): `Scroll response` and
# `Stop latency` with P50/P95/Max, the overlay thread's synchronous work per update, and the cost of
# deriving a window of preview. `Preview 更新延迟` is **not** obtainable here and the report says why
# rather than filling the cell: §23.2 defines it as a comparison of timestamps that the port does not
# carry (`PreviewUpdate` has no `tick`), and its consumer end lives behind the message loop the
# assembly root owns (`P6.08`).
#
# Usage:
#   pwsh -File tools/p5-06-latency.ps1
#   pwsh -File tools/p5-06-latency.ps1 -Runs 5
#   pwsh -File tools/p5-06-latency.ps1 -Out docs/Temp/perf4.jsonl -CrossLens 1500
#
# Task and exit conditions: docs/31 §10 (P5.06). Results are consumed by docs/30 §23.3.
param(
    [string]$Out = '',
    [string[]]$CrossLens = @(),
    [int]$Runs = 3,
    [double]$ThresholdMs = 80.0
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = "docs/Temp/perf4-$(Get-Date -Format yyyy-MM-dd).jsonl" }
$Out = Join-Path $root $Out
$logDir = Join-Path $root 'docs/Temp/perf4-logs'

# `pwsh -File script.ps1 -CrossLens 1500,3840` hands the whole string to one element: `-File` does
# not re-parse arguments as PowerShell syntax (the same trap as P0.03/P0.04/P4.07).
$CrossLens = @($CrossLens | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($CrossLens.Count -eq 0) { $CrossLens = @('1500', '3840') }

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $Out

Write-Host '[P5.06] building the probes (release)' -ForegroundColor Cyan
& cargo test --release -p snapclip-capture --lib --no-run 2>&1 |
    Select-Object -Last 3 | ForEach-Object { Write-Host "  $_" }
if ($LASTEXITCODE -ne 0) { throw "the probes did not build (exit $LASTEXITCODE)" }

# docs/31 exit condition ③ / docs/30 §23.5: the numbers are only comparable if the tree, the
# toolchain and the binary are recorded with them.
$binary = Get-ChildItem (Join-Path $root 'target/release/deps') -Filter 'snapclip_capture-*.exe' |
    Where-Object { $_.Name -notmatch '\.d\.exe$' } | Sort-Object LastWriteTime | Select-Object -Last 1
$header = [ordered]@{
    kind             = 'env'
    date             = (Get-Date).ToString('o')
    profile          = 'release'
    cross_lens       = $CrossLens
    band_rows        = 540
    iterations       = 30
    driver_width     = 1500
    driver_extent    = 900
    driver_stop_ms   = 400
    runs             = $Runs
    threshold_ms     = $ThresholdMs
    render_dip       = '1500x900@120'
    rustc            = (rustc -V)
    cargo            = (cargo -V)
    os               = [System.Environment]::OSVersion.VersionString
    lockfile         = (Get-FileHash -Algorithm SHA256 (Join-Path $root 'Cargo.lock')).Hash
    binary           = if ($binary) { $binary.Name } else { 'unknown' }
    binary_hash      = if ($binary) { (Get-FileHash -Algorithm SHA256 $binary.FullName).Hash } else { 'unknown' }
}
Add-Content -Path $Out -Value ($header | ConvertTo-Json -Compress)

# The probes append their own lines: the hand-off does not go through a pipe (a captured pipe is the
# one thing a Windows sandbox refuses, and the release run has to work in the same places the gate
# does).
$env:SNAPCLIP_PERF4_OUT = $Out

# --- the thumbnail's cost, box filter alone and the whole window refresh ------------------------
$costs = @()
foreach ($cross in $CrossLens) {
    foreach ($kind in @(1, 2)) {
        $name = "cost-kind$kind-$cross"
        $log = Join-Path $logDir "$name.log"
        $env:SNAPCLIP_PERF4_KIND = "$kind"
        $env:SNAPCLIP_PERF4_CROSS_LEN = $cross
        & cargo test --release -p snapclip-capture --lib perf4_measures_the_costs `
            -- --ignored --nocapture --test-threads=1 *> $log
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[P5.06] $name FAILED — see $log" -ForegroundColor Red
            continue
        }
        # `"kind":1,` cannot match `"kind":13,` and `"cross_len":1500,` cannot match 15000: both keys
        # are followed by a comma and the values are never prefixes of each other in this vocabulary.
        $row = Select-String -Path $Out -Pattern ('"kind":' + $kind + ',"cross_len":' + $cross + ',') | Select-Object -Last 1
        if (-not $row) {
            Write-Host "[P5.06] $name produced no result line" -ForegroundColor Yellow
            continue
        }
        $costs += ($row.Line | ConvertFrom-Json)
    }
}

if ($costs.Count -gt 0) {
    Write-Host ''
    Write-Host '[P5.06] preview derivation cost (release, percentiles over 30 iterations)' -ForegroundColor Cyan
    $costs | ForEach-Object {
        $_ | Add-Member -NotePropertyName which -NotePropertyValue $(if ($_.kind -eq 1) { 'box filter' } else { 'window refresh' }) -Force
        $_ | Add-Member -NotePropertyName duty_at_10hz -NotePropertyValue ($_.p95_ns * 10.0 / 1e9) -Force
    }
    $costs | Format-Table which, cross_len, scale, rows, px_per_update,
        @{ n = 'p50_ms'; e = { '{0:N3}' -f ($_.p50_ns / 1e6) } },
        @{ n = 'p95_ms'; e = { '{0:N3}' -f ($_.p95_ns / 1e6) } },
        @{ n = 'max_ms'; e = { '{0:N3}' -f ($_.max_ns / 1e6) } },
        @{ n = 'duty@10Hz'; e = { '{0:P2}' -f $_.duty_at_10hz } } -AutoSize

    # The finding the shape of the two kinds exists to make visible: the refresh is not the filter.
    foreach ($cross in ($costs | ForEach-Object { $_.cross_len } | Sort-Object -Unique)) {
        $band = $costs | Where-Object { $_.cross_len -eq $cross -and $_.kind -eq 1 }
        $window = $costs | Where-Object { $_.cross_len -eq $cross -and $_.kind -eq 2 }
        if ($band -and $window) {
            Write-Host ("[P5.06] cross_len {0}: the window refresh is {1:N2}x the box filter alone (p50 {2:N3} ms vs {3:N3} ms)" -f `
                $cross, ($window.p50_ns / $band.p50_ns), ($window.p50_ns / 1e6), ($band.p50_ns / 1e6))
        }
    }
}

# --- the driver: Scroll Response and Stop latency ------------------------------------------------
$driver = @()
for ($run = 1; $run -le $Runs; $run++) {
    $log = Join-Path $logDir "driver-$run.log"
    & cargo test --release -p snapclip-capture --lib latency_measures_one_run `
        -- --ignored --nocapture --test-threads=1 *> $log
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[P5.06] driver run $run FAILED — see $log" -ForegroundColor Red
        continue
    }
    $row = Select-String -Path $Out -Pattern '"kind":3,' | Select-Object -Last 1
    if ($row) { $driver += ($row.Line | ConvertFrom-Json) }
}

if ($driver.Count -gt 0) {
    Write-Host ''
    Write-Host '[P5.06] closed loop (release, one process per run)' -ForegroundColor Cyan
    $driver | Format-Table steps, injects,
        @{ n = 'scroll_p50_ms'; e = { '{0:N1}' -f ($_.scroll_p50_ns / 1e6) } },
        @{ n = 'scroll_p95_ms'; e = { '{0:N1}' -f ($_.scroll_p95_ns / 1e6) } },
        @{ n = 'scroll_max_ms'; e = { '{0:N1}' -f ($_.scroll_max_ns / 1e6) } },
        @{ n = 'stop_ms'; e = { '{0:N1}' -f ($_.stop_ns / 1e6) } },
        meets_threshold -AutoSize

    $met = @($driver | Where-Object { $_.meets_threshold }).Count
    $worstP95 = ($driver | Measure-Object -Property scroll_p95_ns -Maximum).Maximum
    $worstStop = ($driver | Measure-Object -Property stop_ns -Maximum).Maximum
    $worstP95Ms = $worstP95 / 1e6
    $threshold = $driver[0].threshold_ns / 1e6
    # Reported, not gated, and the report says which: the threshold prices the settle rule at 30 ms
    # while the implemented rule waits `STILL_WINDOW` (40 ms) plus two ticks, so the tail crosses.
    $color = if ($met -eq $driver.Count) { 'Green' } else { 'Yellow' }
    Write-Host ("[P5.06] Scroll response worst p95 {0:N1} ms against §23.3's {1:N0} ms threshold — met in {2} of {3} runs" -f `
        $worstP95Ms, $threshold, $met, $driver.Count) -ForegroundColor $color
    Write-Host ("[P5.06] Stop latency worst {0:N1} ms against §23.3's 60 ms P95 threshold (single sample per run)" -f ($worstStop / 1e6))
}

# --- the consumer: the overlay thread's synchronous work per update ------------------------------
# This half gets its numbers from the case's own stdout: it needs a D3D11 device, which an
# environment variable cannot supply.
$renderLog = Join-Path $logDir 'render.log'
Write-Host ''
Write-Host '[P5.06] the composing thread, four rates' -ForegroundColor Cyan
& cargo test --release -p snapclip-capture --lib the_overlay_thread_never_blocks `
    -- --ignored --nocapture --test-threads=1 *> $renderLog
$renderExit = $LASTEXITCODE
Select-String -Path $renderLog -Pattern '\[P5.06\]' | ForEach-Object { Write-Host "  $($_.Line -replace '^.*\[P5\.06\] ', '')" }
if ($renderExit -ne 0) {
    Write-Host '[P5.06] the consumer case FAILED — see the log above' -ForegroundColor Red
}

# --- what this run cannot say --------------------------------------------------------------------
Write-Host ''
Write-Host '[P5.06] not measured here:' -ForegroundColor Yellow
Write-Host '  Preview 更新延迟 (docs/30 §23.3, P95 ≤ 100 ms): §23.2 defines it against a `PreviewUpdate.tick`'
Write-Host '  timestamp the port does not carry, and its consumer end is behind the message loop the'
Write-Host '  assembly root owns (P6.08). Reported as 未取得 rather than filled in.'

$summary = [ordered]@{
    kind                 = 'summary'
    costs                = @($costs | ForEach-Object { @{ kind = $_.kind; cross_len = $_.cross_len; p50_ns = $_.p50_ns; p95_ns = $_.p95_ns; max_ns = $_.max_ns } })
    driver_runs          = $driver.Count
    driver_meets         = if ($driver.Count -gt 0) { @($driver | Where-Object { $_.meets_threshold }).Count } else { 0 }
    scroll_worst_p95_ns  = if ($driver.Count -gt 0) { ($driver | Measure-Object -Property scroll_p95_ns -Maximum).Maximum } else { 0 }
    stop_worst_ns        = if ($driver.Count -gt 0) { ($driver | Measure-Object -Property stop_ns -Maximum).Maximum } else { 0 }
    render_exit          = $renderExit
    preview_update       = 'not measured: no tick in the port, consumer behind the message loop (P6.08)'
}
Add-Content -Path $Out -Value ($summary | ConvertTo-Json -Compress -Depth 5)

Write-Host ''
Write-Host "[P5.06] measurements in $Out" -ForegroundColor Cyan

if ($driver.Count -eq 0) {
    Write-Host '  no driver run produced a report, so `Scroll response` and `Stop latency` are still 未取得' -ForegroundColor Yellow
    exit 1
}
if ($renderExit -ne 0) { exit 1 }
