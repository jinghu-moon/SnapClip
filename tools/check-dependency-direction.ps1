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
#   powershell ... -File tools/check-dependency-direction.ps1 -Package snapclip-app   # negative control
#
# Exits non-zero and prints every offending edge.

param(
    # Check one package instead of the whole invariant set. Used as the negative control:
    # `-Package snapclip-app` must fail because the shell depends on GPUI and capabilities.
    [string]$Package
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# Capability crates may depend on a shared clipboard service, never on sibling views.
# Pin is now a GPUI feature (docs/52); clipboard/model/history remain UI-free.
$SHELL_ONLY = @("snapclip-app", "tauri", "wry", "gpui", "gpui-pre", "gpui-kit", "gpui-base", "gpui-component", "yue-component")
$APP_ONLY = @("snapclip-app", "tauri", "wry")
$FORBIDDEN = @{
    "snapclip-pin" = $APP_ONLY + @("rusqlite", "arboard", "snapclip-clipboard", "snapclip-capture", "snapclip-history", "snapclip-recognize")
    "snapclip-clipboard" = $SHELL_ONLY + @("rusqlite", "snapclip-pin", "snapclip-capture", "snapclip-history", "snapclip-recognize", "snapclip-ui")
    "snapclip-capture" = $SHELL_ONLY + @("snapclip-pin", "snapclip-clipboard", "rusqlite", "arboard", "snapclip-history", "snapclip-recognize")
    "snapclip-history" = $SHELL_ONLY + @("snapclip-pin", "snapclip-capture", "snapclip-recognize")
    "snapclip-model" = $SHELL_ONLY + @("snapclip-pin", "snapclip-clipboard", "rusqlite", "arboard", "image", "windows", "windows-sys", "snapclip-capture", "snapclip-history", "snapclip-recognize")
}

# `snapclip-model` is the bottom of the graph: serializable value types and pure color
# math (docs/53 section 11.1). `half` represents FP16 without an image codec; its
# cfg-if/zerocopy dependencies are listed explicitly rather than allowing its entire
# transitive graph. UI, platform, codec and capability dependencies remain forbidden.
$MODEL_ALLOWED = @("serde", "serde_core", "serde_derive", "serde_json", "serde_derive_internals", "proc-macro2", "quote", "syn", "unicode-ident", "memchr", "itoa", "ryu", "half", "cfg-if", "zerocopy", "zerocopy-derive", "yue-color")

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

$targets = if ($Package) { @($Package) } else { @("snapclip-capture", "snapclip-history", "snapclip-model", "snapclip-pin", "snapclip-clipboard") }

foreach ($target in $targets) {
    $lines = Get-TreeLines $target
    $names = Get-PackageNames $lines

    $forbidden_for_target = $FORBIDDEN[$target]
    if (-not $forbidden_for_target) {
        # `-Package <other>` (the negative control): apply the strictest list.
        $forbidden_for_target = $SHELL_ONLY + @("snapclip-capture", "snapclip-history", "snapclip-recognize")
    }
    foreach ($dependency in $forbidden_for_target) {
        # The first line of `cargo tree` is the package itself, not a dependency.
        if ($dependency -eq $target) { continue }
        if ($names.Contains($dependency)) {
            $failures.Add("$target depends on $dependency")
        }
    }

    if ($target -eq "snapclip-model") {
        foreach ($name in $names) {
            # The first line of `cargo tree` is the package itself, not a dependency.
            if ($name -eq $target) { continue }
            if ($MODEL_ALLOWED -notcontains $name) {
                $failures.Add("snapclip-model depends on $name (only value types, serialization and the explicit pure-math allowlist are allowed)")
            }
        }
    }

    $count = $names.Count
    Write-Host ("checked {0}: {1} packages in its normal graph" -f $target, $count)
}

# Second pass (docs/30 section 28.4): the scroll/ module is the platform-free core. The whole
# "tests need no desktop" story (docs/30 section 29) rests on it, and no dependency scan can see
# this edge because it is a `use` inside the crate rather than a package. So it is a source scan,
# in the same script and with the same exit code, so that there is one gate and not two.
#
# Skipped under -Package: that switch is the negative control for the dependency pass, and running
# the source scan there would make the control's failure ambiguous.
if (-not $Package) {
    $scrollRoot = "crates/snapclip-capture/src/scroll"
    $scrollFiles = @(Get-ChildItem -Path $scrollRoot -Filter *.rs -Recurse)
    $hits = @($scrollFiles | Select-String -Pattern "crate::windows|crate::sampler|winapi|windows_sys|windows::")
    foreach ($hit in $hits) {
        $relative = Resolve-Path -Relative $hit.Path
        $reference = $hit.Matches[0].Value
        $failures.Add("$relative" + ":" + "$($hit.LineNumber) references " + "$reference (scroll/ must stay platform-free)")
    }
    Write-Host ("checked {0}: {1} files scanned for platform references" -f $scrollRoot, $scrollFiles.Count)
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "dependency direction violated:" -ForegroundColor Red
    foreach ($failure in $failures) { Write-Host "  - $failure" -ForegroundColor Red }
    exit 1
}

Write-Host "dependency direction is clean" -ForegroundColor Green
