param(
    [string]$TargetDir = 'target',
    [string]$CargoConfig = '',
    [string]$Ffmpeg = 'ffmpeg',
    [switch]$Resources,
    [switch]$EnforceStreamingBudget
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $common = @('--locked', '--offline', '--target-dir', $TargetDir)
    if ($CargoConfig) { $common += @('--config', $CargoConfig) }
    python tools/check-hdr-codec-vendor.py
    if ($LASTEXITCODE -ne 0) { throw 'Codec source verification failed' }
    cargo test -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe @common -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'AVIF unit regression failed' }
    cargo clippy -p codec-bench --all-targets --features hdr-avif-probe --no-deps @common -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'AVIF strict Clippy failed' }
    cargo build -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe --release @common
    if ($LASTEXITCODE -ne 0) { throw 'AVIF Release build failed' }
    $probe = Join-Path $TargetDir 'release/hdr-avif-probe.exe'
    python tools/verify-hdr-avif.py --probe $probe --ffmpeg $Ffmpeg
    if ($LASTEXITCODE -ne 0) { throw 'Independent encoder pixel gate failed' }
    python tools/verify-hdr-avif-metadata.py --probe $probe
    if ($LASTEXITCODE -ne 0) { throw 'Dual-colr metadata gate failed' }
    python tools/verify-hdr-avif-matrix.py --probe $probe --ffmpeg $Ffmpeg
    if ($LASTEXITCODE -ne 0) { throw 'Independent decode matrix failed' }
    if ($Resources -or $EnforceStreamingBudget) {
        $limits = @()
        if ($EnforceStreamingBudget) { $limits += '--enforce-streaming-budget' }
        python tools/verify-hdr-avif-resources.py --probe $probe @limits
        if ($LASTEXITCODE -ne 0) { throw 'AVIF resource qualification failed; inspect measurements' }
    }
} finally {
    Pop-Location
}
