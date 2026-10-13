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
#   3. the_entity_map_names_real_symbols - the map between the `entity-map:start` / `entity-map:end`
#      markers hands out one design name per row and says which symbol in which file carries it, and
#      this check holds the map to the tree. It exists because a design role is not a type: V2's
#      `ScrollLoop` was played by `ScrollSession` + `ScrollRuntime` + `ScrollController`, and V3's
#      `Axis`/`Estimate`/`RecoveredImage`/`CaptureLoop` are likewise not type names. Without the map
#      the next reader greps for a name that has never existed.
#      The map is its own list: an earlier revision also compared it against a hard-coded section
#      number in V2, which meant a document renumbering turned the gate red for no defect.
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
# More than one design document, since `docs/34-pin-window-design.md` arrived: `-Doc` takes a
# list, and each document is held to the same three rules. Two additions came with the second one.
#
#   4. the_entity_map_names_real_symbols accepts a row whose symbol is not implemented yet, but
#      only when the row says so in a fourth column - `*(P1 计划)*` or the word `planned`. The
#      distinction is the point: `docs/34` used to name seven symbols that phase P1 would create,
#      and it still names one that a later phase will (`PinCommand`). A
#      gate that failed on them would be red until those phases land, while a gate that ignored
#      them silently would keep ignoring them afterwards. A planned row is counted and printed; a
#      row that neither names a real symbol nor admits it is planned is a failure.
#   5. the same entity is checked per document, not globally: `docs/33` and `docs/34` are allowed
#      to name the same live symbol (`CaptureEvent`), because both genuinely read it.
#
# ASCII only, on purpose: `.githooks/pre-push` invokes this through `powershell`, whose default
# `-File` encoding is ANSI, so the two non-ASCII markers are built from code points below.
# The word for "planned" is built the same way.
#
# Usage:
#   pwsh -NoProfile -File scripts/check-ledger-and-names.ps1
#   pwsh -NoProfile -File scripts/check-ledger-and-names.ps1 -Details

[CmdletBinding()]
param(
    [string[]]$Root = @('crates', 'apps'),
    [string[]]$Doc = @('docs/33-scroll-capture.md', 'docs/34-pin-window-design.md'),
    [string[]]$Ps1Dir = @('scripts', 'tools', 'subfont'),
    [switch]$Details
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# The two pending markers. Built from code points so this file stays ASCII.
$PendingMarkers = @(
    [pscustomobject]@{ Pattern = "$([char]0x5F85)$([char]0x6D4B)"; Name = 'the words for "to be measured"' },
    [pscustomobject]@{ Pattern = "$([char]0x23F3)"; Name = 'the hourglass marker' }
)

# "planned": the word a row has to use to be exempt from the symbol check.
$PlannedWord = "$([char]0x8BA1)$([char]0x5212)"

# V1 names that V2 deleted or merged. `ScrollFrame` is not here: it is alive.
$LegacyNames = @(
    'ScrollTile', 'ScrollExportMeta', 'ScrollArtifactWriter', 'PreviewPatch', 'PreviewState',
    'DriverCommand', 'DriverEvent', 'Alignment', 'Synthetic', 'Adjusting'
)
$LegacyPattern = '\b(' + ($LegacyNames -join '|') + ')\b'

$EntityMapStart = '<!-- entity-map:start -->'
$EntityMapEnd = '<!-- entity-map:end -->'
$DefinitionPattern = '\b(?:struct|enum|trait|type|fn|mod|const|static)\s+{0}\b'

# Every .ps1 this project owns must start with a UTF-8 BOM.
# Windows PowerShell 5.1 decodes a BOM-less file with the ANSI code page, so a
# Chinese comment turns into mojibake, and the parse errors that follow point at
# lines far from the real cause. That cost two debugging rounds on
# scripts/measure-pin-memory.ps1, so it is checked instead of remembered.
# Scope is $Ps1Dir, this project's own scripts: refer/ vendors whole reference
# projects and .comparison-old/ is a frozen copy, and neither is ours to reformat.
function Test-IsUtf8WithBom {
    param([string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return $stream.ReadByte() -eq 0xEF -and $stream.ReadByte() -eq 0xBB -and $stream.ReadByte() -eq 0xBF
    } finally {
        $stream.Dispose()
    }
}

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
# Per document, and the duplicate-name rule is per document too: two design documents may both
# read the same live symbol, but one document may not hand out the same design name twice.
$verified = 0
$mapRows = New-Object System.Collections.Generic.List[object]
$plannedRows = New-Object System.Collections.Generic.List[object]
$docReports = New-Object System.Collections.Generic.List[object]

foreach ($document in $Doc) {
    $docPath = Join-Path $repoRoot $document
    if (-not (Test-Path -LiteralPath $docPath -PathType Leaf)) {
        $failures.Add("the_document_exists: $document is missing")
        continue
    }
    $docLines = Get-Content -LiteralPath $docPath -Encoding UTF8

    # ---- 1. no performance cell still says pending --------------------------------------------
    foreach ($marker in $PendingMarkers) {
        for ($i = 0; $i -lt $docLines.Count; $i++) {
            if ([regex]::IsMatch($docLines[$i], $marker.Pattern)) {
                $failures.Add("no_performance_cell_still_says_pending: ${document}:$($i + 1) carries $($marker.Name)")
            }
        }
    }

    $inMap = $false
    $sawMarkers = $false
    $entityRows = New-Object System.Collections.Generic.List[string]
    $documentRows = 0
    $documentVerified = 0
    for ($i = 0; $i -lt $docLines.Count; $i++) {
        $line = $docLines[$i]
        if ($line.Trim() -eq $EntityMapStart) { $inMap = $true; $sawMarkers = $true; continue }
        if ($line.Trim() -eq $EntityMapEnd) { $inMap = $false; continue }
        if (-not ($inMap -and $line.StartsWith('|'))) { continue }

        $cells = Get-Cells $line
        # The header row names no entity in backticks, and the separator row is dashes.
        if ($cells.Count -lt 3 -or -not $cells[0].Contains('`')) { continue }

        $row = [pscustomobject]@{
            Line     = $i + 1
            Design   = $cells[0]
            Symbols  = $cells[1]
            Location = $cells[2]
            Phase    = if ($cells.Count -ge 4) { $cells[3] } else { '' }
            Document = $document
        }
        $mapRows.Add($row)
        $documentRows++

        $path = $row.Location.Replace('`', '').Trim()
        $file = Join-Path $repoRoot $path
        $symbols = ($row.Symbols.Replace('`', '') -split '\+') | ForEach-Object { $_.Trim() } | Where-Object { $_ }

        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
            if ($row.Phase.Contains($PlannedWord)) {
                $plannedRows.Add([pscustomobject]@{ Document = $document; Line = $row.Line; Design = $row.Design; Reason = "$path is not created yet" })
                continue
            }
            $failures.Add("the_entity_map_names_real_symbols: ${document}:$($row.Line) points at $path, which does not exist")
            continue
        }

        $source = Get-Content -LiteralPath $file -Raw -Encoding UTF8
        foreach ($symbol in $symbols) {
            $pattern = $DefinitionPattern -f [regex]::Escape($symbol)
            if ([regex]::IsMatch($source, $pattern)) { $documentVerified++; $verified++ }
            elseif ($row.Phase.Contains($PlannedWord)) {
                $plannedRows.Add([pscustomobject]@{ Document = $document; Line = $row.Line; Design = $row.Design; Reason = "$path does not define $symbol yet" })
            }
            else {
                $failures.Add("the_entity_map_names_real_symbols: ${document}:$($row.Line) names $symbol, which $path does not define")
            }
        }

        $name = Get-BareName $row.Design
        if ($entityRows -contains $name) {
            $failures.Add("the_entity_map_names_real_symbols: ${document}:$($row.Line) names $name twice")
        }
        else { $entityRows.Add($name) }
    }

    if (-not $sawMarkers) {
        $failures.Add("the_entity_map_names_real_symbols: $document has no entity map ($EntityMapStart .. $EntityMapEnd)")
    }
    if ($documentRows -eq 0) {
        $failures.Add("the_entity_map_names_real_symbols: $document has no entity map rows ($EntityMapStart .. $EntityMapEnd)")
    }
    $docReports.Add([pscustomobject]@{
        Document = $document
        Rows     = $documentRows
        Verified = $documentVerified
        Names    = $entityRows.Count
    })
}

$docList = $Doc -join ', '

# The BOM rule, checked over every script that can run under Windows PowerShell 5.1.
# Each entry is a directory, scanned recursively.
$scannedScripts = @()
foreach ($directory in $Ps1Dir) {
    $scannedScripts += @(Get-ChildItem -Path (Join-Path $repoRoot $directory) -Filter '*.ps1' -File -Recurse -ErrorAction SilentlyContinue)
}
$scannedScripts = @($scannedScripts | Sort-Object FullName -Unique)
foreach ($script in $scannedScripts) {
    if (-not (Test-IsUtf8WithBom $script.FullName)) {
        $relative = $script.FullName.Replace("$repoRoot\", '').Replace('\', '/')
        $failures.Add("script_has_utf8_bom: $relative has no UTF-8 BOM, so Windows PowerShell 5.1 will decode its comments with the ANSI code page")
    }
}

if ($failures.Count -gt 0) {
    Write-Host "[ledger] scanned $($rustFiles.Count) .rs file(s) and $docList"
    Write-Host ''
    foreach ($failure in $failures) { Write-Host "  - $failure" }
    Write-Host ''
    Write-Host "[ledger] $($failures.Count) failure(s): a ledger with a placeholder, or a design that names code that is not there, or a script without a UTF-8 BOM"
    exit 1
}

if ($Details) {
    foreach ($report in $docReports) {
        Write-Host "[ledger] $($report.Document): $($report.Rows) row(s), $($report.Verified) symbol(s) verified, $($report.Names) name(s)"
    }
    Write-Host "[ledger] no document carries a pending marker"
    Write-Host "[ledger] none of the $($LegacyNames.Count) deleted names appears in code"
    Write-Host "[ledger] $($scannedScripts.Count) .ps1 script(s) carry a UTF-8 BOM"
    foreach ($row in $mapRows) { Write-Host "    $($row.Document): $($row.Design) -> $($row.Symbols) in $($row.Location)" }
    if ($plannedRows.Count -gt 0) {
        Write-Host "[ledger] planned, not yet implemented ($($plannedRows.Count)):"
        foreach ($row in $plannedRows) { Write-Host "    $($row.Document): $($row.Design) - $($row.Reason)" }
    }
}

$plannedNote = if ($plannedRows.Count -gt 0) { ", $($plannedRows.Count) row(s) planned" } else { '' }
Write-Host "[ledger] $($mapRows.Count) entity row(s) across $($Doc.Count) document(s) map to $verified symbol(s)$plannedNote; $($LegacyNames.Count) deleted name(s) checked in $($rustFiles.Count) file(s); $($scannedScripts.Count) .ps1 script(s) checked for a UTF-8 BOM; 0 placeholder(s)"
exit 0
