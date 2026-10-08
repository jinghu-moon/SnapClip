# The P0.03 driver: measure the layer-1 matching cost of docs/31 P0.03 (`E-PERF-1`) at the
# three viewports the task fixes, in two layer-1 shapes, one process per scenario, and append
# each result as one JSON line.
#
# Why a script and not a test loop: the task asks for an independent process per scenario so
# that one scenario's allocator high-water mark, page-cache warmth and CPU frequency state
# cannot leak into the next one's timing. `--test-threads=1` inside each process keeps the
# single scenario from competing with itself.
#
# Scope reminder: `P1.02`–`P1.05` do not exist yet, so only the layer-1 prototype is measured
# and combinations ②(1+2) ③(1+2+3) ④(+ORB) are recorded as not obtained, not guessed. The two
# variants are `full` (digest every row each step) and `revealed` (digest only the rows the
# frame revealed and shift the overlapping rows' digests into place).
#
# Usage:
#   pwsh -File tools/p0-03-matching-cost.ps1                                   # all six scenarios
#   pwsh -File tools/p0-03-matching-cost.ps1 -Steps 100 -Viewports 1080p -Variants full
#
# Task and exit conditions: docs/31 §6 (P0.03). Results are consumed by docs/30 §23.3.
param(
    [string]$Out = '',
    [string[]]$Viewports = @(),
    [string[]]$Variants = @(),
    [int]$Steps = 1000,
    [int]$StepRows = 120
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = "docs/Temp/perf1-$(Get-Date -Format yyyy-MM-dd).jsonl" }
$Out = Join-Path $root $Out
$logDir = Join-Path $root 'docs/Temp/perf1-logs'

# `pwsh -File script.ps1 -Viewports a,b` hands the whole `a,b` string to one element, because
# `-File` does not re-parse arguments as PowerShell syntax (the same trap as P0.04).
$Viewports = @($Viewports | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($Viewports.Count -eq 0) { $Viewports = @('1080p', '1440p', '4k') }
$Variants = @($Variants | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($Variants.Count -eq 0) { $Variants = @('full', 'revealed') }

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $Out

Write-Host '[P0.03] building the probe (release)' -ForegroundColor Cyan
& cargo test --release -p snapclip-capture --lib --no-run 2>&1 |
    Select-Object -Last 3 | ForEach-Object { Write-Host "  $_" }
if ($LASTEXITCODE -ne 0) { throw "the probe did not build (exit $LASTEXITCODE)" }

# docs/31 exit condition ③ / docs/30 §23.5: the numbers are only comparable if the tree, the
# toolchain and the binary are recorded with them.
$binary = Get-ChildItem (Join-Path $root 'target/release/deps') -Filter 'snapclip_capture-*.exe' |
    Where-Object { $_.Name -notmatch '\.d\.exe$' } | Sort-Object LastWriteTime | Select-Object -Last 1
$header = [ordered]@{
    kind         = 'env'
    date         = (Get-Date).ToString('o')
    profile      = 'release'
    steps        = $Steps
    step_rows    = $StepRows
    search_rows  = 32
    candidates   = 8
    viewports    = $Viewports
    variants     = $Variants
    rustc        = (rustc -V)
    cargo        = (cargo -V)
    os           = [System.Environment]::OSVersion.VersionString
    lockfile     = (Get-FileHash -Algorithm SHA256 (Join-Path $root 'Cargo.lock')).Hash
    binary       = if ($binary) { $binary.Name } else { 'unknown' }
    binary_hash  = if ($binary) { (Get-FileHash -Algorithm SHA256 $binary.FullName).Hash } else { 'unknown' }
}
Add-Content -Path $Out -Value ($header | ConvertTo-Json -Compress)

$env:SNAPCLIP_PERF1_OUT = $Out
$env:SNAPCLIP_PERF1_STEPS = "$Steps"
$env:SNAPCLIP_PERF1_STEP_ROWS = "$StepRows"

foreach ($viewport in $Viewports) {
    foreach ($variant in $Variants) {
        $log = Join-Path $logDir "$viewport-$variant.log"
        $env:SNAPCLIP_PERF1_VIEWPORT = $viewport
        $env:SNAPCLIP_PERF1_VARIANT = $variant
        $started = Get-Date
        & cargo test --release -p snapclip-capture --lib perf1_measures_one_viewport `
            -- --ignored --nocapture --test-threads=1 *> $log
        $exit = $LASTEXITCODE
        $elapsed = [int]((Get-Date) - $started).TotalSeconds
        if ($exit -ne 0) {
            Write-Host ("[P0.03] {0,-6} {1,-9} FAILED ({2}s, exit {3}) — see {4}" -f $viewport, $variant, $elapsed, $exit, $log) -ForegroundColor Red
            Add-Content -Path $Out -Value (@{ kind = 'failure'; viewport = $viewport; variant = $variant; exit = $exit; log = $log } | ConvertTo-Json -Compress)
            continue
        }
        $row = Select-String -Path $Out -Pattern ('"combo":"layer1-' + [regex]::Escape($variant) + '"') | Select-Object -Last 1
        if ($row) {
            $parsed = $row.Line | ConvertFrom-Json
            Write-Host ("[P0.03] {0,-6} {1,-9} p50 {2,7:N0} us  p95 {3,7:N0} us  max {4,8:N0} us  digest {5,5:N1}%  hit {6,6:P2}  ({7}s)" -f `
                $viewport, $variant, $parsed.p50_us, $parsed.p95_us, $parsed.max_us,
                ($parsed.layers.digest.share * 100), $parsed.top_k_hit_rate, $elapsed)
        }
    }
}

Write-Host ''
Write-Host "[P0.03] measurements in $Out" -ForegroundColor Cyan
Get-Content $Out | Where-Object { $_ -match '"kind":"combo"' } | ForEach-Object { $_ | ConvertFrom-Json } |
    Format-Table combo, viewport, steps, top_k_hit_rate, p50_us, p95_us, max_us,
        @{ n = 'digest_p50'; e = { $_.layers.digest.p50_us } },
        @{ n = 'search_p50'; e = { $_.layers.search_1d.p50_us } },
        @{ n = 'digest_share'; e = { '{0:P1}' -f $_.layers.digest.share } } -AutoSize
