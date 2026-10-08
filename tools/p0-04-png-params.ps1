# The P0.04 driver: measure the twelve PNG compression/filter combinations of docs/31 P0.04,
# one process per combination, and append each result as one JSON line.
#
# Why a script and not a test loop: the point of the experiment is that a previous
# combination's allocator high-water mark, deflate history and page-cache warmth cannot leak
# into the next one's timing. Separate processes is the cheapest way to guarantee that, and
# `--test-threads=1` inside each process keeps the twelve from competing for cores.
#
# Usage:
#   pwsh -File tools/p0-04-png-params.ps1                       # all twelve + the baseline row
#   pwsh -File tools/p0-04-png-params.ps1 -Height 3000 -Runs 1  # a cheap smoke run
#
# Task and exit conditions: docs/31 §6 (P0.04). Results are consumed by docs/30 §17.7.
param(
    [string]$Out = '',
    [int]$Width = 1280,
    [int]$Height = 30000,
    [int]$Runs = 3,
    [string[]]$Combos = @()
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = "docs/Temp/perf2-$(Get-Date -Format yyyy-MM-dd).jsonl" }
$Out = Join-Path $root $Out
$logDir = Join-Path $root 'docs/Temp/perf2-logs'

# `pwsh -File script.ps1 -Combos a,b` hands the whole `a,b` string to one element, because
# `-File` does not re-parse arguments as PowerShell syntax. Split here so both `-Combos a,b`
# and `-Combos a b` mean the same thing.
$Combos = @($Combos | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })

if ($Combos.Count -eq 0) {
    $Combos = @(
        'Fast/NoFilter', 'Fast/Sub', 'Fast/Up', 'Fast/Adaptive',
        'Balanced/NoFilter', 'Balanced/Sub', 'Balanced/Up', 'Balanced/Adaptive',
        'High/NoFilter', 'High/Sub', 'High/Up', 'High/Adaptive',
        'baseline/image'
    )
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Out) | Out-Null
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $Out

# The environment record comes first so a later reader can tell which tree produced the
# numbers. docs/31 P0.03 asks for the same fields (lockfile hash, toolchain) for E-PERF-1.
$header = [ordered]@{
    kind       = 'env'
    date       = (Get-Date).ToString('o')
    profile    = 'release'
    width      = $Width
    height     = $Height
    runs       = $Runs
    rustc      = (rustc -V)
    cargo      = (cargo -V)
    os         = [System.Environment]::OSVersion.VersionString
    lockfile   = (Get-FileHash -Algorithm SHA256 (Join-Path $root 'Cargo.lock')).Hash
    combos     = $Combos
}
Add-Content -Path $Out -Value ($header | ConvertTo-Json -Compress)

# Build once: every combination then runs the same binary and only the env vars differ.
Write-Host '[P0.04] building the probe (release)' -ForegroundColor Cyan
& cargo test --release -p snapclip-app --test png_params_probe --no-run 2>&1 |
    Select-Object -Last 3 | ForEach-Object { Write-Host "  $_" }
if ($LASTEXITCODE -ne 0) { throw "the probe did not build (exit $LASTEXITCODE)" }

$env:SNAPCLIP_PERF2_OUT = $Out
$env:SNAPCLIP_PERF2_WIDTH = "$Width"
$env:SNAPCLIP_PERF2_HEIGHT = "$Height"
$env:SNAPCLIP_PERF2_RUNS = "$Runs"

foreach ($combo in $Combos) {
    $safe = $combo -replace '[^A-Za-z0-9]+', '-'
    $log = Join-Path $logDir "$safe.log"
    $env:SNAPCLIP_PERF2_COMBO = $combo
    $started = Get-Date
    & cargo test --release -p snapclip-app --test png_params_probe `
        perf2_measures_one_parameter_combination -- --ignored --nocapture --test-threads=1 `
        *> $log
    $exit = $LASTEXITCODE
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    if ($exit -ne 0) {
        Write-Host ("[P0.04] {0,-18} FAILED ({1}s, exit {2}) — see {3}" -f $combo, $elapsed, $exit, $log) -ForegroundColor Red
        Add-Content -Path $Out -Value (@{ kind = 'failure'; combo = $combo; exit = $exit; log = $log } | ConvertTo-Json -Compress)
        continue
    }
    $row = Select-String -Path $Out -Pattern ('"combo":"' + [regex]::Escape($combo) + '"') |
        Select-Object -Last 1
    if ($row) {
        $parsed = $row.Line | ConvertFrom-Json
        Write-Host ("[P0.04] {0,-18} p50 {1,7:N0} ms  {2,6:N1} MiB/s  {3,8:N0} KiB  peak {4,6:N1} MiB  ({5}s)" -f `
            $combo, $parsed.p50_ms, $parsed.mib_per_s, ($parsed.png_bytes / 1024), ($parsed.peak_bytes / 1MB), $elapsed)
    }
}

Write-Host ''
Write-Host "[P0.04] measurements in $Out" -ForegroundColor Cyan
Get-Content $Out | Where-Object { $_ -match '"kind":"combo"' } | ForEach-Object { $_ | ConvertFrom-Json } |
    Sort-Object p50_ms | Format-Table combo, p50_ms, max_ms, mib_per_s, png_bytes, peak_bytes, decode_ok,
        decodes_under_default_limits -AutoSize
