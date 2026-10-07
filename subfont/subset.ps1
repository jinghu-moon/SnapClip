# Rebuilds the overlay's embedded UI font subset: the glyphs of **the strings the overlay draws**,
# plus printable ASCII (the panel draws computed text — colour values, coordinates, sizes).
#
# The list of drawn strings is Rust's answer, not something to maintain here: it comes from
# `overlay_drawn_strings()` in win/d2d.rs (assembled from the text's producers), which the step
# below writes to `subfont/drawn-text.txt`. `build_subset.py` then builds the font from it, fails
# if the artifact cannot cover it, guards against a string drawn inline but never listed, and
# installs the result where `include_bytes!` reads it
# (`crates/snapclip-capture/assets/harmonyos-sans-sc-subset.ttf`, which moved there with the
# overlay in docs/23 T1.5) — the old script only wrote `subfont/`, which is why the shipped
# copy had quietly gone stale.
#
# Requires: python3 with fontTools (pip install fonttools) and the source font at
# `refer/HarmonyOS_SansSC_Regular.ttf` (that directory is gitignored).
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
# Keep Python's output readable in Windows PowerShell 5.1 (which decodes the console with the
# system codepage, so UTF-8 prose in an error message arrives as mojibake otherwise).
$env:PYTHONIOENCODING = "utf-8"

cargo test -p snapclip-capture --lib write_drawn_text_for_the_font_subset `
    -- --ignored --nocapture --quiet
if ($LASTEXITCODE -ne 0) { throw "generating subfont/drawn-text.txt failed ($LASTEXITCODE)" }

python3 subfont/build_subset.py
if ($LASTEXITCODE -ne 0) { throw "font subset build failed ($LASTEXITCODE)" }
