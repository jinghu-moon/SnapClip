# Rebuilds the overlay's embedded UI font subset. The work lives in `build_subset.py`, which
# **scans the Rust sources** for string literals (`rust_literals.py`) and takes their characters
# as the required set — there is no list to keep in sync, and no `.ps1` for PowerShell 5.1 to
# mojibake through the system ANSI codepage (which is how the old hand-written list lost glyphs
# without saying anything).
#
# Nothing to remember when adding UI text: write the string in the code, run this script. It fails
# loudly if the produced subset cannot cover what the code draws, and it installs the result where
# `include_bytes!` reads it (`src-tauri/fonts/harmonyos-sans-sc-subset.ttf`) — the old script only
# wrote `subfont/`, which is why the shipped copy had quietly gone stale.
#
# Requires: python3 with fontTools (pip install fonttools) and the source font at
# `refer/HarmonyOS_SansSC_Regular.ttf` (that directory is gitignored).
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

python3 subfont/build_subset.py
if ($LASTEXITCODE -ne 0) { throw "font subset build failed ($LASTEXITCODE)" }
