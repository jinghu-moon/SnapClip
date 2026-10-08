# SnapClip: measures the real test baseline and names every drift. See docs/31 P0.01, §2.6.
#
# Why this exists: `docs/19` claims "475 passed / 9 ignored / 0 failed" and nothing in the
# tree could reproduce it (docs/31 §0.3). A number nobody can re-measure is a legend, and
# "no regression" claims rest on it. This script makes the baseline a fact and makes every
# later drift name the exact tests that moved.
#
# Usage:
#   pwsh -NoProfile -File scripts/record-baseline.ps1                 # measure, write docs/Temp/baseline-<date>.json
#   pwsh -NoProfile -File scripts/record-baseline.ps1 -Out <path>     # measure, write somewhere else
#   pwsh -NoProfile -File scripts/record-baseline.ps1 -Compare <path> # re-measure and name the drift
#
# The artifact is local (`docs/Temp/` is gitignored): it is a measurement, not source.

[CmdletBinding()]
param(
    [string]$Out,
    [string]$Compare
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$crates = @('snapclip-app', 'snapclip-capture', 'snapclip-history', 'snapclip-model')

function Get-CargoOutput {
    param([string[]]$Arguments, [switch]$IncludeStderr)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($IncludeStderr) {
            # cargo reports diagnostics on stderr (test results go to stdout), so a warning
            # count taken from stdout alone is silently always zero.
            return ,@(& cargo @Arguments 2>&1 | ForEach-Object { "$_" })
        }
        # cargo writes its report to stdout; stderr carries the progress lines we do not need.
        return ,@(& cargo @Arguments 2>$null)
    } finally {
        $ErrorActionPreference = $previous
    }
}

function Measure-Crate {
    param([string]$Crate)

    $lines = Get-CargoOutput -Arguments @('test', '-p', $Crate, '--lib')
    $passed = 0; $failed = 0; $ignored = 0
    $results = [System.Collections.Generic.List[string]]::new()
    foreach ($line in $lines) {
        if ($line -match '^test result:\s+(\w+)\.\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored') {
            $results.Add($line.Trim())
            $passed += [int]$Matches[2]
            $failed += [int]$Matches[3]
            $ignored += [int]$Matches[4]
        }
    }

    $listLines = Get-CargoOutput -Arguments @('test', '-p', $Crate, '--lib', '--', '--list')
    $names = [System.Collections.Generic.List[string]]::new()
    foreach ($line in $listLines) {
        if ($line -match '^(?<name>[^\s].*): test$') {
            $names.Add($Matches['name'])
        }
    }
    $names.Sort()

    if ($names.Count -ne $passed + $failed + $ignored) {
        Write-Warning ("{0}: --list reported {1} tests but `cargo test` reported {2}; the two runs disagree" -f $Crate, $names.Count, ($passed + $failed + $ignored))
    }

    return [ordered]@{
        passed  = $passed
        failed  = $failed
        ignored = $ignored
        results = $results
        names   = $names
    }
}

function Measure-Check {
    $lines = Get-CargoOutput -Arguments @('check', '--workspace', '--all-targets') -IncludeStderr
    $warnings = [System.Collections.Generic.List[object]]::new()
    $errors = 0
    $message = $null
    foreach ($line in $lines) {
        if ($line -match '^error(\[E\d+\])?: (?<text>.+)$') {
            if ($line -notmatch 'could not compile' -and $line -notmatch '^\s*$') {
                $errors += 1
                $message = $Matches['text']
            }
        } elseif ($line -match '^warning: (?<text>.+)$') {
            $message = $Matches['text']
        } elseif ($line -match '^\s+-->\s+(?<location>.+)$' -and $null -ne $message) {
            if ($message -notmatch 'generated \d+ warning' -and $message -notmatch '^\d+ warning') {
                $warnings.Add([ordered]@{ message = $message; location = $Matches['location'] })
            }
            $message = $null
        }
    }
    return [ordered]@{ errors = $errors; warnings = $warnings }
}

function Measure-Baseline {
    # Returns the measurement as data. Diagnostics are printed by the caller: anything this
    # function wrote to the output stream would be captured along with the object.
    $baseline = [ordered]@{}
    $total = [ordered]@{ passed = 0; failed = 0; ignored = 0 }
    foreach ($crate in $crates) {
        $measured = Measure-Crate $crate
        $baseline[$crate] = $measured
        $total.passed += $measured.passed
        $total.failed += $measured.failed
        $total.ignored += $measured.ignored
    }
    $check = Measure-Check

    $commit = (& git -C $repoRoot rev-parse HEAD) 2>$null
    return [ordered]@{
        date     = (Get-Date).ToString('yyyy-MM-dd')
        recorded = (Get-Date).ToString('s')
        commit   = ("$commit").Trim()
        rustc    = ((& rustc --version) 2>$null) -join ''
        crates   = $baseline
        total    = $total
        check    = $check
    }
}

function Write-BaselineSummary {
    param([object]$Measured)
    foreach ($crate in $crates) {
        $entry = $Measured.crates[$crate]
        Write-Output ("[baseline] {0}: {1} passed / {2} failed / {3} ignored" -f $crate, $entry.passed, $entry.failed, $entry.ignored)
    }
    Write-Output ("[baseline] cargo check --workspace --all-targets: {0} error, {1} warning(s)" -f $Measured.check.errors, @($Measured.check.warnings).Count)
    foreach ($warning in @($Measured.check.warnings)) {
        Write-Output ("[baseline]   warning: {0}  ({1})" -f $warning.message, $warning.location)
    }
    Write-Output ("[baseline] total: {0} passed / {1} failed / {2} ignored" -f $Measured.total.passed, $Measured.total.failed, $Measured.total.ignored)
}

function Compare-Baseline {
    param([string]$Path, [object]$Measured)

    if (-not (Test-Path $Path -PathType Leaf)) {
        Write-Error ("no baseline artifact at {0}: there is nothing to compare against, so nothing is proven. Run this script without -Compare first (docs/31 P0.01 RED)." -f $Path)
        exit 1
    }

    $reference = Get-Content $Path -Raw | ConvertFrom-Json
    $drift = [System.Collections.Generic.List[string]]::new()

    foreach ($crate in $crates) {
        $before = $reference.crates.$crate
        $after = $Measured.crates[$crate]
        if ($null -eq $before) {
            $drift.Add(("{0}: absent from the baseline" -f $crate))
            continue
        }
        foreach ($counter in @('passed', 'failed', 'ignored')) {
            $delta = $after[$counter] - $before.$counter
            if ($delta -ne 0) {
                $drift.Add(("{0}: {1} {2} -> {3} ({4:+#;-#;0})" -f $crate, $counter, $before.$counter, $after[$counter], $delta))
            }
        }

        # docs/31 §2.6: a delta that cannot name a test is a regression, so name them here.
        $removed = @($before.names | Where-Object { $after.names -notcontains $_ })
        $added = @($after.names | Where-Object { $before.names -notcontains $_ })
        foreach ($name in $removed) { $drift.Add(("{0}: test disappeared: {1}" -f $crate, $name)) }
        foreach ($name in $added) { $drift.Add(("{0}: test appeared: {1}" -f $crate, $name)) }
    }

    $beforeWarnings = @($reference.check.warnings)
    $afterWarnings = @($Measured.check.warnings)
    if ($beforeWarnings.Count -ne $afterWarnings.Count) {
        $drift.Add(("cargo check: {0} warning(s) -> {1}" -f $beforeWarnings.Count, $afterWarnings.Count))
    }
    foreach ($warning in $afterWarnings) {
        if (-not ($beforeWarnings | Where-Object { $_.message -eq $warning.message -and $_.location -eq $warning.location })) {
            $drift.Add(("cargo check: new warning: {0}  ({1})" -f $warning.message, $warning.location))
        }
    }

    if ($drift.Count -gt 0) {
        Write-Output '[baseline] drift against the recorded baseline:'
        foreach ($entry in $drift) { Write-Output ("  - {0}" -f $entry) }
        Write-Error 'baseline drifted; every entry above has to be explained in the commit message before it counts as intentional (docs/31 §2.6)'
        exit 1
    }

    Write-Output ("[baseline] no drift against {0}" -f $Path)
    exit 0
}

$measured = Measure-Baseline
Write-BaselineSummary -Measured $measured

if ($Compare) {
    $comparePath = if ([System.IO.Path]::IsPathRooted($Compare)) { $Compare } else { Join-Path $repoRoot $Compare }
    Compare-Baseline -Path $comparePath -Measured $measured
} else {
    $target = if ($Out) { $Out } else { "docs/Temp/baseline-$($measured.date).json" }
    if (-not [System.IO.Path]::IsPathRooted($target)) { $target = Join-Path $repoRoot $target }
    $targetDirectory = Split-Path -Parent $target
    if (-not (Test-Path $targetDirectory)) { New-Item -ItemType Directory -Force -Path $targetDirectory | Out-Null }
    $measured | ConvertTo-Json -Depth 6 | Set-Content -Path $target -Encoding utf8
    Write-Output ("[baseline] recorded to {0}" -f $target)
    exit 0
}

