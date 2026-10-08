# The P0.03 driver: measure the matching cost of docs/31 P0.03 (`E-PERF-1`) at the three
# viewports the task fixes, one process per scenario, and append each result as one JSON line.
#
# Why a script and not a test loop: the task asks for an independent process per scenario so
# that one scenario's allocator high-water mark, page-cache warmth and CPU frequency state
# cannot leak into the next one's timing. `--test-threads=1` inside each process keeps the
# single scenario from competing with itself.
#
# Two sets of scenarios:
#
#   layer1  the `P0.03` prototype measurement, two shapes: `full` (digest every row each step)
#           and `revealed` (digest only the rows the frame revealed and shift the overlapping
#           rows' digests into place). This is the attribution for the first layer, and it is
#           what the 2026-10-08 six-scenario run recorded.
#   funnel  the four combinations docs/31 P0.03 asks for, over the *shipped* pipeline:
#           `digest` (attribution only: the two `primary_digests` calls of layer 1),
#           `l1` (digest + 1D candidates), `l12` (+ layer 2), `l123` (+ layer 3),
#           `l123-orb` (+ the ORB second opinion, quoted per vote × trigger rate).
#           The prior is the scripted step (this measures cost, not accuracy) and the search
#           window is the production formula `max(4, ceil(0.3·|expected|))`.
#
# Usage:
#   pwsh -File tools/p0-03-matching-cost.ps1                                    # everything
#   pwsh -File tools/p0-03-matching-cost.ps1 -Sets funnel -Combos l123 -Viewports 4k
#   pwsh -File tools/p0-03-matching-cost.ps1 -Sets layer1 -Steps 100 -Variants full
#
# Task and exit conditions: docs/31 §6 (P0.03). Results are consumed by docs/30 §23.3.
param(
    [string]$Out = '',
    [string[]]$Viewports = @(),
    [string[]]$Variants = @(),
    [string[]]$Combos = @(),
    [string[]]$Sets = @(),
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
$Combos = @($Combos | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($Combos.Count -eq 0) { $Combos = @('digest', 'l1', 'l12', 'l123', 'l123-orb') }
$Sets = @($Sets | ForEach-Object { $_ -split ',' } | Where-Object { $_ -ne '' })
if ($Sets.Count -eq 0) { $Sets = @('layer1', 'funnel') }

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
    search_rows  = 'production: max(4, ceil(0.3*step_rows))'
    candidates   = 8
    sets         = $Sets
    viewports    = $Viewports
    variants     = $Variants
    combos       = $Combos
    orb_votes    = 8
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

function Invoke-Scenario {
    param([string]$Viewport, [string]$Label, [string]$TestName, [hashtable]$Env, [string]$Pattern)
    $log = Join-Path $logDir "$Viewport-$Label.log"
    $env:SNAPCLIP_PERF1_VIEWPORT = $Viewport
    foreach ($key in $Env.Keys) { Set-Item -Path "env:$key" -Value $Env[$key] }
    $started = Get-Date
    & cargo test --release -p snapclip-capture --lib $TestName `
        -- --ignored --nocapture --test-threads=1 *> $log
    $exit = $LASTEXITCODE
    $elapsed = [int]((Get-Date) - $started).TotalSeconds
    if ($exit -ne 0) {
        Write-Host ("[P0.03] {0,-6} {1,-10} FAILED ({2}s, exit {3}) — see {4}" -f $Viewport, $Label, $elapsed, $exit, $log) -ForegroundColor Red
        Add-Content -Path $Out -Value (@{ kind = 'failure'; viewport = $Viewport; scenario = $Label; exit = $exit; log = $log } | ConvertTo-Json -Compress)
        return
    }
    $row = Select-String -Path $Out -Pattern $Pattern | Select-Object -Last 1
    if ($row) { return ($row.Line | ConvertFrom-Json) }
    Write-Host ("[P0.03] {0,-6} {1,-10} produced no result line ({2}s)" -f $Viewport, $Label, $elapsed) -ForegroundColor Yellow
    return $null
}

foreach ($viewport in $Viewports) {
    if ($Sets -contains 'layer1') {
        foreach ($variant in $Variants) {
            $parsed = Invoke-Scenario -Viewport $viewport -Label $variant `
                -TestName 'perf1_measures_one_viewport' `
                -Env @{ SNAPCLIP_PERF1_VARIANT = $variant } `
                -Pattern ('"combo":"layer1-' + [regex]::Escape($variant) + '"')
            if ($parsed) {
                Write-Host ("[P0.03] {0,-6} {1,-10} p50 {2,7:N0} us  p95 {3,7:N0} us  max {4,8:N0} us  digest {5,5:N1}%  hit {6,6:P2}" -f `
                    $viewport, $variant, $parsed.p50_us, $parsed.p95_us, $parsed.max_us,
                    ($parsed.layers.digest.share * 100), $parsed.top_k_hit_rate)
            }
        }
    }
    if ($Sets -contains 'funnel') {
        foreach ($combo in $Combos) {
            $parsed = Invoke-Scenario -Viewport $viewport -Label $combo `
                -TestName 'perf1_measures_one_funnel_combo' `
                -Env @{ SNAPCLIP_PERF1_COMBO = $combo } `
                -Pattern ('"combo":"funnel-' + [regex]::Escape($combo) + '"')
            if ($parsed) {
                Write-Host ("[P0.03] {0,-6} {1,-10} p50 {2,8:N0} us  p95 {3,8:N0} us  max {4,9:N0} us  confirmed {5,6:P1}  wrong {6,3}  orb/vote {7,7:N0} us  trig {8,6:P1}" -f `
                    $viewport, $combo, $parsed.p50_us, $parsed.p95_us, $parsed.max_us,
                    $parsed.confirmed_rate, $parsed.wrong, $parsed.layers.orb.p50_us, $parsed.orb_trigger_rate)
            }
        }
    }
}

Write-Host ''
Write-Host "[P0.03] measurements in $Out" -ForegroundColor Cyan
$rows = @(Get-Content $Out | Where-Object { $_ -match '"kind":"combo"' } | ForEach-Object { $_ | ConvertFrom-Json })
if ($rows.Count -eq 0) {
    Write-Host '  (no combo lines)' -ForegroundColor Yellow
} else {
    $layer1 = @($rows | Where-Object { $_.combo -like 'layer1-*' })
    $funnel = @($rows | Where-Object { $_.combo -like 'funnel-*' })
    if ($layer1.Count -gt 0) {
        $layer1 | Format-Table combo, viewport, steps, top_k_hit_rate, p50_us, p95_us, max_us,
            @{ n = 'digest_p50'; e = { $_.layers.digest.p50_us } },
            @{ n = 'search_p50'; e = { $_.layers.search_1d.p50_us } },
            @{ n = 'digest_share'; e = { '{0:P1}' -f $_.layers.digest.share } } -AutoSize
    }
    if ($funnel.Count -gt 0) {
        $funnel | Format-Table combo, viewport, steps, confirmed_rate, wrong, p50_us, p95_us, max_us,
            @{ n = 'l1_p50'; e = { $_.layers.layer1.p50_us } },
            @{ n = 'l2_p50'; e = { $_.layers.layer2.p50_us } },
            @{ n = 'l3_p50'; e = { $_.layers.layer3.p50_us } },
            @{ n = 'orb_p50'; e = { $_.layers.orb.p50_us } },
            @{ n = 'orb_trigger'; e = { '{0:P1}' -f $_.orb_trigger_rate } },
            @{ n = 'peak_MiB'; e = { '{0:N1}' -f ($_.alloc.peak_bytes / 1MB) } } -AutoSize
    }
}
