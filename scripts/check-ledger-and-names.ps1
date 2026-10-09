#!/usr/bin/env pwsh
# SnapClip: the performance table has no placeholder left, and the design names the code.
#
# docs/31-scroll-capture-tdd-tasklist.md P6.08 (V2 sections 23.3 and 2.2). Three failure modes,
# and all three are the same failure: a document that was true when it was written.
#
#   1. no_performance_cell_still_says_pending - section 23.3 is the table that says what the
#      numbers have to be. Every cell is now either a measurement with its experiment named, or
#      the words that say it was not obtained and why. A leftover placeholder is how a table that
#      has been filled in still reads as if it had not: "to be measured" next to a number that
#      has existed for two stages.
#   2. no_legacy_entity_name_survives - V1's entity sketch (docs/19) named types that V2 deleted
#      or merged: `ScrollTile`, `ScrollExportMeta`, `ScrollArtifactWriter`, `PreviewPatch`,
#      `PreviewState`, `DriverCommand`, `DriverEvent`, `Alignment`, `Synthetic`, `Adjusting`.
#      A deleted name that survives in code is a second vocabulary, and two names grow two logics
#      (D-6). Comments are exempt on purpose - a comment that says "`Adjusting` was removed" is
#      the record of a decision, not a second definition - so the code text is stripped first.
#   3. the_entity_map_names_real_symbols - section 2.2 hands out ten names; the map in 2.2.1 makes
#      each one point at a symbol in a file, and this check holds the map to the tree. It exists
#      because `ScrollLoop` is a design role and not a type: the code calls it `ScrollSession` +
#      `ScrollRuntime` + `ScrollController`, and without the map the next reader greps for a name
#      that has never existed.
#
# `ScrollFrame` is deliberately NOT a legacy name: it is alive - one delivered frame of the
# window-level capture path, read back once, in `crates/snapclip-capture/src/windows/providers.rs`.
# The task list grouped it with the deleted names, and a gate that had copied that list would have
# been red forever, which is worse than no gate.
#
# Known boundaries, on purpose: it reads section 23.3 by scanning the whole design document (a
# placeholder anywhere in it is a placeholder); it strips `//` comments and string literals but not
# `/* */` blocks; it checks that the map's symbols are defined in the file the map names, not that
# the map is complete; and it cannot see a placeholder written in words no one listed.
#
# ASCII only, on purpose: `.githooks/pre-push` invokes this through `powershell`, whose default
# `-File` encoding is ANSI, so the two non-ASCII markers are built from code points below.
#
# Usage:
#   pwsh -NoProfile -File scripts/check-ledger-and-names.ps1
#   pwsh -NoProfile -File scripts/check-ledger-and-names.ps1 -Details

[CmdletBinding()]
param(
    [string[]]$Root = @('crates', 'apps'),
    [string]$Doc = 'docs/30-scroll-capture-design-v2.md',
    [switch]$Details
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# The two pending markers. Built from code points so this file stays ASCII.
$PendingMarkers = @(
    [pscustomobject]@{ Pattern = "$([char]0x5F85)$([char]0x6D4B)"; Name = 'the words for "to be measured"' },
    [pscustomobject]@{ Pattern = "$([char]0x23F3)"; Name = 'the hourglass marker' }
)

# V1 names that V2 deleted or merged. `ScrollFrame` is not here: it is alive.
$LegacyNames = @(
    'ScrollTile', 'ScrollExportMeta', 'ScrollArtifactWriter', 'PreviewPatch', 'PreviewState',
    'DriverCommand', 'DriverEvent', 'Alignment', 'Synthetic', 'Adjusting'
)
$LegacyPattern = '\b(' + ($LegacyNames -join '|') + ')\b'

$EntityMapStart = '<!-- entity-map:start -->'
$EntityMapEnd = '<!-- entity-map:end -->'
$DefinitionPattern = '\b(?:struct|enum|trait|type|fn|mod|const|static)\s+{0}\b'

function Get-CodeText {
    param([string]$Line)
    # String literals first (a state name in a string is data), then the line comment.
    $code = [regex]::Replace($Line, '"(?:[^"\\]|\\.)*"', '""')
    $cut = $code.IndexOf('//')
    if ($cut -ge 0) { $code = $code.Substring(0, $cut) }
    return $code
}

function Get-RustFiles {
    $files = @()
    foreach ($scanRoot in $Root) {
        $parent = Join-Path $repoRoot (Join-Path $scanRoot '*')
        foreach ($crate in Get-ChildItem -Path $parent -Directory -ErrorAction SilentlyContinue) {
            foreach ($part in @('src', 'tests')) {
                $directory = Join-Path $crate.FullName $part
                if (Test-Path -LiteralPath $directory -PathType Container) {
                    foreach ($file in Get-ChildItem -Path $directory -Recurse -Filter '*.rs' -File) {
                        $files += $file.FullName
                    }
                }
            }
        }
    }
    return $files
}

function Get-Cells {
    param([string]$Line)
    $parts = $Line.Split('|')
    if ($parts.Count -lt 3) { return @() }
    $cells = @()
    for ($i = 1; $i -lt $parts.Count - 1; $i++) { $cells += $parts[$i].Trim() }
    return $cells
}

function Get-BareName {
    param([string]$Cell)
    $text = $Cell
    # Drop a parenthetical gloss ("`RecoveredImage`（画布）") and every backtick.
    $text = [regex]::Replace($text, '[\(\uff08].*$', '')
    $text = $text.Replace('`', '')
    return $text.Trim()
}

$failures = New-Object System.Collections.Generic.List[string]
$docPath = Join-Path $repoRoot $Doc
if (-not (Test-Path -LiteralPath $docPath -PathType Leaf)) {
    Write-Host "[ledger] the design document is missing: $Doc"
    exit 1
}
$docLines = Get-Content -LiteralPath $docPath -Encoding UTF8

# ---- 1. no performance cell still says pending ------------------------------------------------
foreach ($marker in $PendingMarkers) {
    for ($i = 0; $i -lt $docLines.Count; $i++) {
        if ([regex]::IsMatch($docLines[$i], $marker.Pattern)) {
            $failures.Add("no_performance_cell_still_says_pending: ${Doc}:$($i + 1) carries $($marker.Name)")
        }
    }
}

# ---- 2. no legacy entity name survives in code ------------------------------------------------
$rustFiles = Get-RustFiles
foreach ($file in $rustFiles) {
    $lines = Get-Content -LiteralPath $file -Encoding UTF8
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $code = Get-CodeText $lines[$i]
        if ([regex]::IsMatch($code, $LegacyPattern)) {
            $relative = $file.Replace("$repoRoot\", '').Replace('\', '/')
            $name = [regex]::Match($code, $LegacyPattern).Value
            $failures.Add("no_legacy_entity_name_survives: ${relative}:$($i + 1) uses $name, which V2 deleted")
        }
    }
}

# ---- 3. the entity map names real symbols -----------------------------------------------------
$mapRows = New-Object System.Collections.Generic.List[object]
$inMap = $false
$sawMarkers = $false
$entityRows = New-Object System.Collections.Generic.List[string]
$inEntityTable = $false
for ($i = 0; $i -lt $docLines.Count; $i++) {
    $line = $docLines[$i]
    if ($line.Trim() -eq $EntityMapStart) { $inMap = $true; $sawMarkers = $true; continue }
    if ($line.Trim() -eq $EntityMapEnd) { $inMap = $false; continue }
    if ($inMap -and $line.StartsWith('|')) {
        $cells = Get-Cells $line
        # The header row names no entity in backticks, and the separator row is dashes.
        if ($cells.Count -ge 3 -and $cells[0].Contains('`')) {
            $mapRows.Add([pscustomobject]@{ Line = $i + 1; Design = $cells[0]; Symbols = $cells[1]; Location = $cells[2] })
        }
    }
    if (-not $inEntityTable -and $line -match '^### 2\.2 ') { $inEntityTable = $true; continue }
    if ($inEntityTable) {
        if ($line.StartsWith('|')) {
            $cells = Get-Cells $line
            # Section 2.2 names every entity in backticks; the header does not.
            if ($cells.Count -ge 1 -and $cells[0].Contains('`')) {
                $name = Get-BareName $cells[0]
                if ($name) { $entityRows.Add($name) }
            }
        }
        elseif ($line.Trim() -eq '' -and $entityRows.Count -gt 0) { $inEntityTable = $false }
    }
}

if (-not $sawMarkers) {
    $failures.Add("the_entity_map_names_real_symbols: ${Doc} has no entity map ($EntityMapStart .. $EntityMapEnd)")
}
if ($entityRows.Count -eq 0) {
    $failures.Add("the_entity_map_names_real_symbols: ${Doc} has no section 2.2 entity table to compare against")
}

$verified = 0
foreach ($row in $mapRows) {
    $path = $row.Location.Replace('`', '').Trim()
    $file = Join-Path $repoRoot $path
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        $failures.Add("the_entity_map_names_real_symbols: ${Doc}:$($row.Line) points at $path, which does not exist")
        continue
    }
    $source = Get-Content -LiteralPath $file -Raw -Encoding UTF8
    $symbols = ($row.Symbols.Replace('`', '') -split '\+') | ForEach-Object { $_.Trim() } | Where-Object { $_ }
    foreach ($symbol in $symbols) {
        $pattern = $DefinitionPattern -f [regex]::Escape($symbol)
        if (-not [regex]::IsMatch($source, $pattern)) {
            $failures.Add("the_entity_map_names_real_symbols: ${Doc}:$($row.Line) names $symbol, which $path does not define")
        }
        else { $verified++ }
    }
    if ($entityRows -notcontains (Get-BareName $row.Design)) {
        $failures.Add("the_entity_map_names_real_symbols: ${Doc}:$($row.Line) maps $($row.Design), which section 2.2 does not name")
    }
}
foreach ($name in $entityRows) {
    $mapped = $false
    foreach ($row in $mapRows) { if ((Get-BareName $row.Design) -eq $name) { $mapped = $true } }
    if (-not $mapped) {
        $failures.Add("the_entity_map_names_real_symbols: section 2.2 names $name, and the map does not")
    }
}

if ($failures.Count -gt 0) {
    Write-Host "[ledger] scanned $($rustFiles.Count) .rs file(s) and $Doc"
    Write-Host ''
    foreach ($failure in $failures) { Write-Host "  - $failure" }
    Write-Host ''
    Write-Host "[ledger] $($failures.Count) failure(s): a ledger with a placeholder, or a design that names code that is not there"
    exit 1
}

if ($Details) {
    Write-Host "[ledger] $Doc carries no pending marker"
    Write-Host "[ledger] none of the $($LegacyNames.Count) deleted names appears in code"
    foreach ($row in $mapRows) { Write-Host "    $($row.Design) -> $($row.Symbols) in $($row.Location)" }
}
Write-Host "[ledger] $($entityRows.Count) entity name(s) map to $verified symbol(s); $($LegacyNames.Count) deleted name(s) checked in $($rustFiles.Count) file(s); 0 placeholder(s)"
exit 0
