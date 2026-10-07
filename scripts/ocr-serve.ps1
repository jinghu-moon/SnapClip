# 启动 rapidocr serve 本地评估台（模型目录默认 OCR-Model\serve）。
#
# 用法：
#   .\scripts\ocr-serve.ps1                       # 直接跑已构建的 release 二进制
#   .\scripts\ocr-serve.ps1 -Reverify             # 启动期冷验证全部模型
#   .\scripts\ocr-serve.ps1 -Port 9000            # 换端口
#   .\scripts\ocr-serve.ps1 -- --log-level flow   # -- 之后的参数原样透传给 serve
#
# 二进制不存在时自动 cargo build（需要 --features "cli-default serve" 的构建）。
param(
    [string]$ModelDir = "$PSScriptRoot\..\OCR-Model\serve",
    [int]$Port = 0,
    [switch]$Reverify,
    [Parameter(ValueFromRemainingArguments)]$Rest
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root 'crates\rapid-ocr-rs\target\release\rapidocr.exe'

if (-not (Test-Path $exe)) {
    Write-Host 'rapidocr.exe (release, serve feature) not found; building...'
    cargo build --release --features "cli-default serve" -p rapid-ocr-rs --bin rapidocr
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$serveArgs = @('serve', '--model-dir', (Resolve-Path $ModelDir).Path)
if ($Port -gt 0)    { $serveArgs += @('--port', "$Port") }
if ($Reverify)      { $serveArgs += '--reverify-models' }
if ($Rest) {
    # 去掉用户自己传的 `--` 分隔符，剩余参数原样透传
    $serveArgs += @($Rest | Where-Object { $_ -ne '--' })
}

Write-Host "http://127.0.0.1:$(if ($Port -gt 0) { $Port } else { 8760 })"
& $exe @serveArgs
