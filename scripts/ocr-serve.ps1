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
    [string]$ModelDir,
    [int]$Port = 0,
    [switch]$Reverify,
    [Parameter(ValueFromRemainingArguments)]$Rest
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
# $PSScriptRoot 在 param 默认值表达式里不可用，只能在体内补默认
if (-not $ModelDir) { $ModelDir = Join-Path $root 'OCR-Model\serve' }
$exe = Join-Path $root 'crates\rapid-ocr-rs\target\release\rapidocr.exe'

# 页面与 serve 源码都是编译期嵌入（include_str!），比 exe 新就必须重建，
# 否则服务的是旧版页面（改完 index.html 看不到效果的典型根因）。
$needBuild = -not (Test-Path $exe)
if (-not $needBuild) {
    $exeTime = (Get-Item $exe).LastWriteTime
    $embedded = Join-Path $root 'crates\rapid-ocr-rs\src\bin\web\index.html'
    if ((Get-Item $embedded).LastWriteTime -gt $exeTime) { $needBuild = $true }
}
if ($needBuild) {
    Write-Host 'rapidocr.exe missing or embedded sources newer; building...'
    # rapid-ocr-rs 是独立仓库（根 workspace 里被 exclude），只能在自己目录里构建，不能用 -p
    Push-Location (Join-Path $root 'crates\rapid-ocr-rs')
    try { cargo build --release --features "cli-default serve" --bin rapidocr }
    finally { Pop-Location }
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
