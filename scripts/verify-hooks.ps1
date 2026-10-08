# SnapClip: verifies that the push gate is installed and usable. See docs/31 §4.3.
#
# Discipline that is not visible does not exist (docs/31 §2.4, D-14): "pushing is
# trustworthy" has to be a mechanical fact, so this script fails loudly when the hook
# is missing, unmounted, or would be unreadable by `sh`.
#
# Usage: pwsh -NoProfile -File scripts/verify-hooks.ps1

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$hookPath = Join-Path $repoRoot '.githooks/pre-push'
$failures = [System.Collections.Generic.List[string]]::new()

if (-not (Test-Path $hookPath -PathType Leaf)) {
    $failures.Add("the hook file does not exist: .githooks/pre-push")
} else {
    $bytes = [System.IO.File]::ReadAllBytes($hookPath)
    $text = [System.Text.Encoding]::UTF8.GetString($bytes)

    # A CRLF hook dies with "bad interpreter" before it can gate anything, and
    # core.autocrlf is true in this repository (see .gitattributes).
    if ($text.Contains("`r`n")) {
        $failures.Add('the hook contains CRLF line endings; it must be LF-only (.gitattributes pins this)')
    }
    if (-not $text.StartsWith('#!/bin/sh')) {
        $failures.Add('the hook does not start with a #!/bin/sh shebang')
    }
    foreach ($command in @('cargo check --workspace --all-targets', 'cargo test --workspace --lib', 'tools/check-dependency-direction.ps1')) {
        if (-not $text.Contains($command)) {
            $failures.Add("the hook does not run '$command'")
        }
    }
}

$configured = (& git config --get core.hooksPath) 2>$null
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($configured)) {
    $failures.Add('core.hooksPath is unset, so git runs no hook at all (git config core.hooksPath .githooks)')
} elseif ($configured.Trim() -ne '.githooks') {
    $failures.Add("core.hooksPath is '$($configured.Trim())', not '.githooks'")
}

if ($failures.Count -gt 0) {
    Write-Error ("the push gate is not in place:`n  - " + ($failures -join "`n  - "))
    exit 1
}

Write-Output '[verify-hooks] .githooks/pre-push exists, is LF-only, and core.hooksPath = .githooks'
exit 0
