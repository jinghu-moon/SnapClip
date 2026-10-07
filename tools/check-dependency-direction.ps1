# Dependency-direction gate (docs/23 T1.9).
#
# The whole point of splitting the capability crates is that the dependency arrows only
# ever point one way:
#
#   snapclip  ──►  snapclip-capture  ──►  snapclip-model
#   (shell)        snapclip-history  ──►  snapclip-model
#                  snapclip-recognize ──► snapclip-model
#
# `cargo tree` is the cheapest way to prove it, and it is the only check that catches the
# accidental edges a refactor like this one is prone to: capture held a real one into the
# clipboard module until T1.4/T1.5 moved it out.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-dependency-direction.ps1
#   powershell ... -File tools/check-dependency-direction.ps1 -Package snapclip   # negative control
#
# Exits non-zero and prints every offending edge.

param(
    # Check one package instead of the whole invariant set. Used as the negative control:
    # `-Package snapclip` must fail, because the shell *does* depend on Tauri.
    [string]$Package
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# Per crate: names that may never appear in its normal dependency graph.
#
# The list differs by crate on purpose. `rusqlite` is forbidden in capture (screenshots do
# not touch the database) but is exactly what history is for; `arboard` is forbidden in
# both because owning the clipboard belongs to the shell. Capability crates never depend
# on each other: the shell wires them.
$SHELL_ONLY = @("tauri", "wry", "gpui", "gpui-kit")
$FORBIDDEN = @{
    "snapclip-capture" = $SHELL_ONLY + @("rusqlite", "arboard", "snapclip-history", "snapclip-recognize")
    "snapclip-history" = $SHELL_ONLY + @("arboard", "snapclip-capture", "snapclip-recognize")
    "snapclip-model"   = $SHELL_ONLY + @("rusqlite", "arboard", "image", "windows", "windows-sys", "snapclip-capture", "snapclip-history", "snapclip-recognize")
}

# `snapclip-model` is the bottom of the graph: value types and `serde`, nothing else.
$MODEL_ALLOWED = @("serde", "serde_core", "serde_derive", "serde_json", "serde_derive_internals", "proc-macro2", "quote", "syn", "unicode-ident", "memchr", "itoa", "ryu")

function Get-TreeLines([string]$target) {
    $output = & cargo tree -p $target -e normal 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo tree -p $target failed:`n$($output -join "`n")"
    }
    return @($output)
}

# `cargo tree` prints each package as `… ├── name v1.2.3` (optionally with ` (*)`).
function Get-PackageNames([string[]]$lines) {
    $names = New-Object System.Collections.Generic.HashSet[string]
    foreach ($line in $lines) {
        if ($line -match '(?:^|[\s└├│─]+)\s*([a-zA-Z0-9_-]+) v[0-9]') {
            [void]$names.Add($Matches[1])
        }
    }
    return $names
}

$failures = New-Object System.Collections.Generic.List[string]

$targets = if ($Package) { @($Package) } else { @("snapclip-capture", "snapclip-history", "snapclip-model") }

foreach ($target in $targets) {
    $lines = Get-TreeLines $target
    $names = Get-PackageNames $lines

    $forbidden_for_target = $FORBIDDEN[$target]
    if (-not $forbidden_for_target) {
        # `-Package <other>` (the negative control): apply the strictest list.
        $forbidden_for_target = $SHELL_ONLY + @("snapclip-capture", "snapclip-history", "snapclip-recognize")
    }
    foreach ($forbidden in $forbidden_for_target) {
        # The first line of `cargo tree` is the package itself, not a dependency.
        if ($forbidden -eq $target) { continue }
        if ($names.Contains($forbidden)) {
            $failures.Add("$target depends on $forbidden")
        }
    }

    if ($target -eq "snapclip-model") {
        foreach ($name in $names) {
            # The first line of `cargo tree` is the package itself, not a dependency.
            if ($name -eq $target) { continue }
            if ($MODEL_ALLOWED -notcontains $name) {
                $failures.Add("snapclip-model depends on $name (only serde is allowed)")
            }
        }
    }

    $count = $names.Count
    Write-Host ("checked {0}: {1} packages in its normal graph" -f $target, $count)
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "dependency direction violated:" -ForegroundColor Red
    foreach ($failure in $failures) { Write-Host "  - $failure" -ForegroundColor Red }
    exit 1
}

Write-Host "dependency direction is clean" -ForegroundColor Green
