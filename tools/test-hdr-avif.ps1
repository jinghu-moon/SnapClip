param(
    [string]$TargetDir = 'target',
    [string]$CargoConfig = '',
    [string]$Ffmpeg = 'ffmpeg',
    [string]$Ffprobe = '',
    [switch]$Resources,
    [switch]$EnforceStreamingBudget,
    [switch]$GridResources,
    [switch]$BandResources,
    [switch]$SpoolResources
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $common = @('--locked', '--offline', '--target-dir', $TargetDir)
    if ($CargoConfig) { $common += @('--config', $CargoConfig) }
    python tools/check-hdr-codec-vendor.py
    if ($LASTEXITCODE -ne 0) { throw 'Codec source verification failed' }
    $licenseArgs = @()
    if ($CargoConfig) { $licenseArgs += @('--config', $CargoConfig) }
    python tools/check-hdr-codec-licenses.py @licenseArgs
    if ($LASTEXITCODE -ne 0) { throw 'Codec source/license inventory drifted' }
    cargo test -p snapclip-avif @common -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Shared product codec regression failed' }
    cargo test -p snapclip-app -p snapclip-pin --lib --features snapclip-app/avif @common -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Production AVIF save/preview regression failed' }
    cargo test -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe @common -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'AVIF unit regression failed' }
    cargo clippy -p codec-bench --all-targets --features hdr-avif-probe --no-deps @common -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'AVIF strict Clippy failed' }
    cargo build -p codec-bench --bin hdr-avif-probe --features hdr-avif-probe --release @common
    if ($LASTEXITCODE -ne 0) { throw 'AVIF Release build failed' }
    $probe = Join-Path $TargetDir 'release/hdr-avif-probe.exe'
    if (-not $Ffprobe) {
        $ffmpegDirectory = Split-Path -Parent $Ffmpeg
        if ($ffmpegDirectory) { $Ffprobe = Join-Path $ffmpegDirectory 'ffprobe.exe' }
        else { $Ffprobe = 'ffprobe' }
    }
    python tools/verify-hdr-avif.py --probe $probe --ffmpeg $Ffmpeg
    if ($LASTEXITCODE -ne 0) { throw 'Independent encoder pixel gate failed' }
    python tools/verify-hdr-avif-metadata.py --probe $probe
    if ($LASTEXITCODE -ne 0) { throw 'Dual-colr metadata gate failed' }
    python tools/verify-hdr-avif-matrix.py --probe $probe --ffmpeg $Ffmpeg
    if ($LASTEXITCODE -ne 0) { throw 'Independent decode matrix failed' }
    python tools/verify-hdr-avif-product.py --probe $probe
    if ($LASTEXITCODE -ne 0) { throw 'Shared product color/alpha or adversarial resource gate failed' }
    python tools/verify-hdr-avif-grid.py --probe $probe --ffmpeg $Ffmpeg --ffprobe $Ffprobe
    if ($LASTEXITCODE -ne 0) { throw 'Independent grid/alpha/ICC verification failed' }
    python tools/verify-hdr-avif-bands.py --probe $probe --ffmpeg $Ffmpeg --ffprobe $Ffprobe
    if ($LASTEXITCODE -ne 0) { throw 'Independent FP16 band/master color verification failed' }
    if ($SpoolResources) {
        python tools/verify-hdr-avif-spool.py --probe $probe --ffmpeg $Ffmpeg --ffprobe $Ffprobe
        if ($LASTEXITCODE -ne 0) { throw 'Disk-spooled large-output memory/format gate failed' }
    }
    if ($BandResources) {
        python tools/verify-hdr-avif-resources.py --probe $probe --bands --enforce-streaming-budget
        if ($LASTEXITCODE -ne 0) { throw 'Band/master resource gate failed' }
    }
    if ($GridResources) {
        python tools/verify-hdr-avif-resources.py --probe $probe --grid --enforce-streaming-budget
        if ($LASTEXITCODE -ne 0) { throw 'Bounded grid resource gate failed' }
        foreach ($size in @(@(3840, 2160), @(8256, 5504))) {
            & $probe --grid-noise-resources $size[0] $size[1]
            if ($LASTEXITCODE -ne 0) { throw 'Grid noise exceeded its bounded-refusal contract' }
        }
    }
    if ($Resources -or $EnforceStreamingBudget) {
        $limits = @()
        if ($EnforceStreamingBudget) { $limits += '--enforce-streaming-budget' }
        python tools/verify-hdr-avif-resources.py --probe $probe @limits
        if ($LASTEXITCODE -ne 0) { throw 'AVIF resource qualification failed; inspect measurements' }
    }
} finally {
    Pop-Location
}
