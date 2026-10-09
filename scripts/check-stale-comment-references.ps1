#!/usr/bin/env pwsh
# SnapClip: no comment may point at a module that does not exist.
#
# docs/31-scroll-capture-tdd-tasklist.md P6.06 (D-15, docs/30 section 33.1). A comment is a
# negative asset: a refactor changes the code and leaves the prose behind, and the next reader
# follows a name that is not there. `crate::application::capture_service` was four tasks out of
# date before anyone noticed, because nothing ever checked a comment.
#
# Three rules, all mechanical and narrow on purpose:
#
#   1. for every `crate::a::b::...` in a comment, every segment except the last must resolve to a
#      module of the crate the file belongs to. The last segment is exempt because it is normally
#      an item (`crate::artifact::CaptureService`), and a segment that starts with an upper-case
#      letter ends the chain for the same reason (`...::GraphicsDevice::create`).
#   2. `platform::...` is dead on sight: no crate in this workspace has a `platform` module (the
#      Win32 layer is `crate::windows`), so any such path is a leftover from the old layout.
#   3. `src-tauri/` is dead on sight: that tree was deleted, so a path into it - a file, a
#      directory, a module - only misleads. A sentence about history keeps its meaning without
#      the path ("extracted out of the previous shell").
#
# Prose can still be wrong in ways no script can see. P6.06 found and fixed sites that name a
# collaborator instead of a path ("Tauri event publication" after the Tauri shell was deleted,
# "the Vue toolbar" after the Vue frontend was deleted, "the only GPU -> CPU transfer" after the
# readback became lazy); a script that recognised those would be guessing at English. What it
# holds is the mechanical half: a path in a comment resolves or the gate fails.
#
# Known boundaries, on purpose: it scans `crates/*/src`, `apps/*/src` and each crate's own
# `Cargo.toml`; it reads line comments (`//` for Rust, `#` for TOML) and skips `/* */` blocks; it
# does not inspect `#[cfg]`-gated modules any differently from the rest; and it does not know the
# name of a crate that a comment merely mentions (`snapclip-history`'s service layer is prose).
#
# ASCII only, on purpose: `.githooks/pre-push` invokes this through `powershell`, whose default
# `-File` encoding is ANSI, so a non-ASCII string literal would arrive mangled.
#
# Usage:
#   pwsh -NoProfile -File scripts/check-stale-comment-references.ps1
#   pwsh -NoProfile -File scripts/check-stale-comment-references.ps1 -Details

[CmdletBinding()]
param(
    [string[]]$Root = @('crates', 'apps'),
    [switch]$Details
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot

# A `crate::` path in a comment. Multi-line paths (`crate::a::
# b`) are rare enough that a line-local match is the honest version.
$PathPattern = 'crate::[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*'

# Rule 2 and rule 3: a module root and a tree this repository does not have.
$DeadRootPattern = '(?<![A-Za-z0-9_:])platform::[A-Za-z_][A-Za-z0-9_:]*'
$DeadTreePattern = 'src-tauri/'

function Get-CommentText {
    param([string]$Line, [string]$Style = 'rs')
    # Strip string literals first: `"crate::x"` in a doc example is data, not a reference.
    $code = [regex]::Replace($Line, '"(?:[^"\\]|\\.)*"', '""')
    if ($Style -eq 'toml') {
        if ($code -match '^\s*#\s?(.*)$') { return $Matches[1] }
        return ''
    }
    if ($code -match '^\s*(///|//!|//|\*)\s?(.*)$') { return $Matches[2] }
    return ''
}

function Test-ModuleExists {
    param([string]$Directory, [string]$Segment)
    $file = Join-Path $Directory "$Segment.rs"
    if (Test-Path -LiteralPath $file -PathType Leaf) { return $true }
    $nested = Join-Path (Join-Path $Directory $Segment) 'mod.rs'
    return (Test-Path -LiteralPath $nested -PathType Leaf)
}

function Get-CrateSourceRoot {
    param([string]$File)
    $directory = Split-Path -Parent $File
    while ($directory -and $directory.Length -ge $repoRoot.Length) {
        if (Test-Path -LiteralPath (Join-Path $directory 'Cargo.toml') -PathType Leaf) {
            return (Join-Path $directory 'src')
        }
        $parent = Split-Path -Parent $directory
        if ($parent -eq $directory) { break }
        $directory = $parent
    }
    return $null
}

function Add-Stale {
    param([System.Collections.Generic.List[string]]$Into, [string]$File, [int]$Line, [string]$Text)
    $relative = $File.Replace("$repoRoot\", '').Replace('\', '/')
    $Into.Add("${relative}:${Line} $Text")
}

function Get-StaleReferences {
    param([string]$File, [string]$Style)
    $sourceRoot = Get-CrateSourceRoot $File
    if (-not $sourceRoot) { return @() }
    $stale = New-Object System.Collections.Generic.List[string]
    $lines = Get-Content -LiteralPath $File
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $comment = Get-CommentText -Line $lines[$i] -Style $Style
        if (-not $comment) { continue }
        foreach ($match in [regex]::Matches($comment, $PathPattern)) {
            $segments = $match.Value -split '::'
            $directory = $sourceRoot
            $ok = $true
            # Skip segment 0 (`crate`); the last segment is an item, and an upper-case segment
            # ends the chain wherever it appears.
            for ($s = 1; $s -lt $segments.Count - 1; $s++) {
                $segment = $segments[$s]
                if ($segment -cmatch '^[A-Z]') { break }
                if (-not (Test-ModuleExists -Directory $directory -Segment $segment)) {
                    $ok = $false
                    break
                }
                $directory = Join-Path $directory $segment
            }
            if (-not $ok) { Add-Stale -Into $stale -File $File -Line ($i + 1) -Text $match.Value }
        }
        foreach ($match in [regex]::Matches($comment, $DeadRootPattern)) {
            Add-Stale -Into $stale -File $File -Line ($i + 1) -Text $match.Value
        }
        foreach ($match in [regex]::Matches($comment, $DeadTreePattern)) {
            Add-Stale -Into $stale -File $File -Line ($i + 1) -Text "a path under $($match.Value)"
        }
    }
    return $stale
}

$files = @()
# PowerShell is case-insensitive, so the loop variable must not be called `$root`: that is `$Root`.
foreach ($scanRoot in $Root) {
    $src = Join-Path $repoRoot (Join-Path $scanRoot '*')
    foreach ($crate in Get-ChildItem -Path $src -Directory -ErrorAction SilentlyContinue) {
        $sourceDirectory = Join-Path $crate.FullName 'src'
        if (Test-Path -LiteralPath $sourceDirectory -PathType Container) {
            foreach ($file in Get-ChildItem -Path $sourceDirectory -Recurse -Filter '*.rs' -File) {
                $files += [pscustomobject]@{ Path = $file.FullName; Style = 'rs' }
            }
        }
        # A Cargo.toml comment is a comment: D-15's own list included one.
        $manifest = Join-Path $crate.FullName 'Cargo.toml'
        if (Test-Path -LiteralPath $manifest -PathType Leaf) {
            $files += [pscustomobject]@{ Path = $manifest; Style = 'toml' }
        }
    }
}

$stale = New-Object System.Collections.Generic.List[string]
foreach ($file in $files) {
    foreach ($entry in (Get-StaleReferences -File $file.Path -Style $file.Style)) {
        $stale.Add($entry)
    }
}

if ($stale.Count -gt 0) {
    Write-Host "[comments] scanned $($files.Count) file(s) under $($Root -join ', ')"
    Write-Host ''
    Write-Host 'comments that name a module which does not exist (D-15, docs/30 section 33.1):'
    foreach ($entry in $stale) { Write-Host "  - $entry" }
    Write-Host ''
    Write-Host "[comments] $($stale.Count) stale reference(s); each one sends the next reader to a name that is not there"
    exit 1
}

if ($Details) {
    Write-Host '[comments] every path in a comment resolves to something that exists'
}
Write-Host "[comments] scanned $($files.Count) file(s): 0 stale reference(s)"
exit 0
