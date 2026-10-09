#!/usr/bin/env pwsh
# SnapClip: no test may decide at run time that it has nothing to assert.
#
# docs/31-scroll-capture-tdd-tasklist.md P6.05 (D-14, docs/30 section 29.2). The defect is not
# "a test skipped" - it is "a test skipped and nobody could tell". `cargo test` prints nothing for
# a passing test, so the `eprintln!("skipping: ...")` that accompanies a silent `return` is
# invisible in every normal run, and `cargo test -- --nocapture` is the only way to see it.
#
# A test may skip, but only in one of two visible shapes:
#   (a) #[ignore = "why it needs an environment; how to restore it"]  - static, greppable, and
#       `cargo test` reports it in the ignored column;
#   (b) an explicit environment assertion - the test fails loudly when the environment is absent
#       (`expect`/`assert!`), which keeps it in the default gate on machines that have one.
# What is left over is a `return` that can be reached without asserting anything first.
#
# Only **unmarked** tests are counted, which is the metric the task names: an `#[ignore]`d probe
# is already out of the default gate, and the operator who runs it with `--ignored` has asked for
# it (several probes deliberately return after printing a diagnosis - see P3.10, docs/31 §14.3).
#
# The rule is "a return is only honest after an assertion" rather than "a test may not return
# early": a polling loop may `return` after an assertion (`detection_worker.rs` does), and a test
# that has already asserted something has not passed by accident. What the rule catches is the
# guard that decides before anything has been checked - which is every environment guard there is.
#
# Known boundary, on purpose: the sibling defect "print a warning and fall through" (a `match` arm
# that does `Err(error) => eprintln!("... unavailable ...")` and lets the test continue) has no
# `return` for this script to find. A rule of the shape "a test body with no assertion at all that
# printed something" would misfire on legitimate smoke tests which only promise not to panic (there
# is one in snapclip-history), so it is not implemented. The P6.05 sweep fixed the three real
# instances by hand (bitblt.rs and two in providers.rs); they are recorded in docs/30 section 29.2.
#
# ASCII only, on purpose: `.githooks/pre-push` invokes this through `powershell`, whose default
# `-File` encoding is ANSI, so a non-ASCII string literal would arrive mangled.
#
# Usage:
#   pwsh -NoProfile -File scripts/count-unignored-desktop-tests.ps1
#   pwsh -NoProfile -File scripts/count-unignored-desktop-tests.ps1 -Details

[CmdletBinding()]
param(
    [string[]]$Root = @('crates', 'apps'),
    [switch]$Details
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# An assertion is what makes an early exit honest. `unwrap`/`expect` count: they assert that a
# setup step succeeded.
$AssertionPattern = '(assert(_eq|_ne|_matches)?!|debug_assert|panic!|unreachable!|todo!|expect\(|unwrap\(|unwrap_or_else)'

# Strip string literals and trailing line comments before looking at braces: test bodies are full
# of format strings whose `{}` would otherwise unbalance the counter.
function Get-CodeText {
    param([string]$Line)
    $code = [regex]::Replace($Line, '"(?:[^"\\]|\\.)*"', '""')
    $code = [regex]::Replace($code, '//.*$', '')
    return $code
}

function Get-BodyEnd {
    param([string[]]$Lines, [int]$Start)
    $depth = 0
    $seen = $false
    for ($i = $Start; $i -lt $Lines.Count; $i++) {
        $code = Get-CodeText $Lines[$i]
        foreach ($ch in $code.ToCharArray()) {
            if ($ch -eq '{') { $depth++; $seen = $true }
            elseif ($ch -eq '}') { $depth-- }
        }
        if ($seen -and $depth -le 0) { return $i }
    }
    return $Lines.Count - 1
}

# The `return` is honest when the test has already asserted something before it. The window is the
# whole body up to that point rather than the enclosing block: the two early exits that are not
# environment guards (`unit.rs`'s "UIA exposed no level", `detection_worker.rs`'s polling loop) both
# follow an assertion, and a test that has asserted once cannot pass while asserting nothing.
function Test-ReturnFollowsAssertion {
    param([string]$Body, [int]$At)
    return [bool]([regex]::IsMatch($Body.Substring(0, $At), $AssertionPattern))
}

$violations = [System.Collections.Generic.List[string]]::new()
$reasonlessIgnores = [System.Collections.Generic.List[string]]::new()
$testCount = 0
$ignoreCount = 0
$fileCount = 0

foreach ($name in $Root) {
    $rootPath = Join-Path $repoRoot $name
    if (-not (Test-Path $rootPath)) { continue }
    $files = Get-ChildItem -Path $rootPath -Recurse -Filter *.rs -File |
        Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' } |
        Sort-Object FullName

    foreach ($file in $files) {
        $fileCount++
        $relative = $file.FullName.Substring($repoRoot.Length + 1)
        $lines = [System.IO.File]::ReadAllLines($file.FullName)
        $testLine = 0
        $ignoreLine = 0
        $ignoreHasReason = $false
        $i = 0
        while ($i -lt $lines.Count) {
            $trimmed = $lines[$i].Trim()

            if ($trimmed -match '^#\[test\]$') {
                $testLine = $i + 1
                $i++
                continue
            }

            if ($trimmed -match '^#\[ignore(\s*=\s*".*")?\]$') {
                $ignoreCount++
                $ignoreLine = $i + 1
                $ignoreHasReason = $trimmed.StartsWith('#[ignore = ')
                if (-not $ignoreHasReason) {
                    $reasonlessIgnores.Add("${relative}:$($i + 1)")
                }
                $i++
                continue
            }

            if ($testLine -gt 0 -and $trimmed -match '^(async\s+)?fn\s+(\w+)') {
                $testName = $Matches[2]
                $end = Get-BodyEnd -Lines $lines -Start $i
                $body = ($lines[$i..$end] | ForEach-Object { Get-CodeText $_ }) -join "`n"
                $testCount++

                if ($ignoreLine -eq 0) {
                    $returnLines = [System.Collections.Generic.List[int]]::new()
                    $from = 0
                    while ($true) {
                        $at = $body.IndexOf('return;', $from)
                        if ($at -lt 0) { break }
                        if (-not (Test-ReturnFollowsAssertion -Body $body -At $at)) {
                            $prefix = $body.Substring(0, $at)
                            $returnLines.Add($testLine + ($prefix -split "`n").Count - 1)
                        }
                        $from = $at + 1
                    }
                    if ($returnLines.Count -gt 0) {
                        $where = ($returnLines | ForEach-Object { ':' + $_ }) -join ', '
                        $violations.Add("${relative}:$testLine $testName reaches a return without asserting ($($returnLines.Count) of them, at $where)")
                    }
                }

                $i = $end + 1
                $testLine = 0
                $ignoreLine = 0
                $ignoreHasReason = $false
                continue
            }

            $i++
        }
    }
}

Write-Host "[desktop-tests] scanned $fileCount .rs file(s): $testCount #[test] function(s), $ignoreCount #[ignore]"

if ($violations.Count -gt 0 -or $reasonlessIgnores.Count -gt 0) {
    Write-Host ''
    if ($violations.Count -gt 0) {
        Write-Host "tests that can pass without asserting (D-14, docs/30 section 29.2):"
        foreach ($violation in $violations) { Write-Host "  - $violation" }
        Write-Host '  make each one #[ignore = "why; restore: how"] + expect(...), or assert the environment'
    }
    if ($reasonlessIgnores.Count -gt 0) {
        Write-Host ''
        Write-Host "#[ignore] attributes without a reason:"
        foreach ($entry in $reasonlessIgnores) { Write-Host "  - $entry" }
    }
    Write-Host ''
    Write-Host "[desktop-tests] $($violations.Count) unmarked desktop test(s), $($reasonlessIgnores.Count) #[ignore] without a reason"
    exit 1
}

if ($Details) {
    Write-Host '[desktop-tests] every #[test] either runs everywhere or says why it does not'
}
Write-Host '[desktop-tests] 0 unmarked desktop test(s), 0 #[ignore] without a reason'
exit 0
