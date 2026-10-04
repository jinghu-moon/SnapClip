$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# Encoding-proof: the CJK glyphs and x are expressed as ASCII codepoints (U+xxxx),
# never as literal Chinese inside this file. Windows PowerShell 5.1 reads a no-BOM
# .ps1 using the system ANSI codepage, so literal 中文 passed through --text gets
# mojibaked and those glyphs are silently dropped from the subset (=> tofu in the
# info panel). Keep the character set as U+ codes to avoid that entirely.
#
# Coverage: the ASCII the info panel / size label draw (space # % ( ) , - digits,
# the specific upper/lower-case letters) + x (U+00D7) + the shortcut CJK
# 色 值 格 式 复 制 坐 标 模 滚 轮 缩 放.
# Adding a NEW CJK glyph means appending its U+ code here.
#
# Current set (59 codepoints):
#   space # % ( ) , -
#   0-9
#   A B C D E F G H L P R S X Y
#   a b c d e f g h l p r s x y
#   x(00D7)
#   色 值 格 式 复 制 坐 标 模 滚 轮 缩 放
$unicodes = "U+0020,U+0023,U+0025,U+0028,U+0029,U+002C,U+002D,U+0030-0039,U+0041-0048,U+004C,U+0050,U+0052-0053,U+0058-0059,U+0061-0068,U+006C,U+0070,U+0072-0073,U+0078-0079,U+00D7,U+8272,U+503C,U+683C,U+5F0F,U+590D,U+5236,U+5750,U+6807,U+6A21,U+6EDA,U+8F6E,U+7F29,U+653E"

New-Item -ItemType Directory -Force -Path subfont | Out-Null
$output = "subfont/harmonyos-sans-sc-subset.ttf"

# ---------------------------------------------------------------------------
# Pass 1: subset + aggressively drop every table we do not need.
#   --layout-features=""        strip all OpenType features (no GSUB/GPOS work)
#   --no-hinting                drop cvt/fpgm/prep and per-glyph hint bytecode
#   --desubroutinize            flatten CFF subroutines (mostly a no-op for TTF)
#   --name-IDs="1,2"            keep only family + subfamily name records
#   --name-languages="0x409"    keep only the US-English name record
#   --drop-tables+=...          remove everything the renderer never reads
#   --notdef-glyph              keep .notdef so missing glyphs render as tofu
#   --recommended-glyphs        keep the handful of glyphs FreeType/Windows expect
# ---------------------------------------------------------------------------
python3 -m fontTools.subset "refer/HarmonyOS_SansSC_Regular.ttf" `
    --unicodes="$unicodes" `
    --output-file="$output" `
    --layout-features="" `
    --no-hinting `
    --desubroutinize `
    --name-IDs="1,2" `
    --name-languages="0x409" `
    --drop-tables+=DSIG,hdmx,VDMX,LTSH,PCLT,gasp,meta,kern,GPOS,GSUB,GDEF,BASE,JSTF,MATH,prep,fpgm,cvt `
    --notdef-glyph `
    --recommended-glyphs

$ttfBytes = (Get-Item $output).Length
$ttfKB = [math]::Round($ttfBytes / 1KB, 1)

# ---------------------------------------------------------------------------
# Pass 2 (optional): compress the TTF so include_bytes! embeds a smaller blob.
# The Rust side decompresses it back into memory before handing it to the font
# parser, so no WOFF2 support is needed.
#
# Choose the backend:
#   "brotli" -> smallest output.  Needs:  python3 -m pip install brotli
#               Rust:  brotli::Decompressor::new(FONT, 4096).read_to_end(&mut ttf)
#   "zlib"   -> stdlib only, ~15-20% larger than brotli.
#               Rust:  flate2::read::ZlibDecoder::new(FONT).read_to_end(&mut ttf)
#   "none"   -> skip compression, embed the raw .ttf directly.
#
# If the chosen compressor is missing or fails, Pass 2 is skipped and the raw
# .ttf produced by Pass 1 is kept, so the script never hard-fails here.
# ---------------------------------------------------------------------------
$compressor = "brotli"          # "brotli" | "zlib" | "none"

$compressed = $null
switch ($compressor) {
    "brotli" { $compressed = "subfont/harmonyos-sans-sc-subset.ttf.br" }
    "zlib"   { $compressed = "subfont/harmonyos-sans-sc-subset.ttf.z"  }
}

if ($compressor -ne "none") {
    try {
        if ($compressor -eq "brotli") {
            python3 -c "import brotli, sys; open(sys.argv[2], 'wb').write(brotli.compress(open(sys.argv[1], 'rb').read(), quality=11))" $output $compressed
        } else {
            python3 -c "import zlib, sys; open(sys.argv[2], 'wb').write(zlib.compress(open(sys.argv[1], 'rb').read(), 9))" $output $compressed
        }
        $brBytes = (Get-Item $compressed).Length
        $brKB = [math]::Round($brBytes / 1KB, 1)
        $ratio = [math]::Round(100 * $brBytes / $ttfBytes, 1)
        Write-Output "wrote $compressed  ($brKB KB, $ratio% of TTF)"
    } catch {
        Write-Warning "compressor '$compressor' failed: $($_.Exception.Message)"
        Write-Warning "keeping raw .ttf only (skip Pass 2)."
    }
}

# ---------------------------------------------------------------------------
# Ship the artifacts into the crate asset dir that include_bytes! embeds:
#
#   Copy-Item subfont/harmonyos-sans-sc-subset.ttf    src-tauri/fonts/harmonyos-sans-sc-subset.ttf    -Force
#   Copy-Item subfont/harmonyos-sans-sc-subset.ttf.br src-tauri/fonts/harmonyos-sans-sc-subset.ttf.br -Force
#   # or, if using zlib:
#   Copy-Item subfont/harmonyos-sans-sc-subset.ttf.z  src-tauri/fonts/harmonyos-sans-sc-subset.ttf.z  -Force
# ---------------------------------------------------------------------------

Write-Output "wrote $output  ($ttfKB KB)"