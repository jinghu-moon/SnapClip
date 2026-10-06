# Rebuilds the overlay's embedded UI font subset. The work lives in `build_subset.py` —
# Python reads `drawn-glyphs.txt` as UTF-8, so the glyph list is the UI's own strings and
# PowerShell's ANSI-without-BOM .ps1 handling can no longer silently drop CJK glyphs from the
# subset (which is how the info panel ended up with half its text in a fallback font).
#
# Adding UI text: put the string in `subfont/drawn-glyphs.txt`. The script fails loudly if the
# produced subset cannot cover it, and it installs the result where `include_bytes!` reads it
# (`src-tauri/fonts/harmonyos-sans-sc-subset.ttf`) — the old script only wrote `subfont/`, which
# is why the shipped copy had quietly gone stale.
#
# Requires: python3 with fontTools (pip install fonttools) and the source font at
# `refer/HarmonyOS_SansSC_Regular.ttf` (that directory is gitignored).
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

python3 subfont/build_subset.py
if ($LASTEXITCODE -ne 0) { throw "font subset build failed ($LASTEXITCODE)" }
