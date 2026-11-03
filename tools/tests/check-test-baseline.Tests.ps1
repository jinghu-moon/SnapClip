#!/usr/bin/env pwsh
# Exercise the gate with synthetic cargo output; no desktop or clipboard access.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$gate = Join-Path $root 'tools/check-test-baseline.ps1'
$directory = Join-Path $root ('target/baseline-tests-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$floors = [ordered]@{ snapclip_app = 267; snapclip_pin = 58; snapclip_capture = 354; snapclip_history = 43; snapclip_clipboard = 8; snapclip_model = 23 }
function Check-Case([string]$name, [hashtable]$counts, [bool]$passes) {
    $log = Join-Path $directory "$name.log"
    $lines = foreach ($stem in $counts.Keys) {
        "     Running unittests src/lib.rs (target/debug/deps/$stem-0123456789abcdef.exe)"
        "test result: ok. $($counts[$stem]) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
    }
    $lines | Set-Content -LiteralPath $log -Encoding utf8
    $output = & powershell -NoProfile -File $gate -Log $log 2>&1
    if (($LASTEXITCODE -eq 0) -ne $passes) { throw "$name produced the wrong gate result`n$output" }
    Write-Host "$name passed"
}
try {
    $counts = @{}; foreach ($key in $floors.Keys) { $counts[$key] = $floors[$key] }
    Check-Case 'preserve-moved-tests' $counts $true
    $counts.snapclip_clipboard = 7
    Check-Case 'reject-lost-clipboard-test' $counts $false
    $counts.snapclip_clipboard = 9; $counts.snapclip_history = 42
    Check-Case 'reject-history-loss-despite-extra-clipboard-test' $counts $false
    $counts.snapclip_history = 43; $counts.Remove('snapclip_clipboard')
    Check-Case 'reject-missing-clipboard-crate' $counts $false
} finally {
    Remove-Item -LiteralPath $directory -Recurse -Force
}
