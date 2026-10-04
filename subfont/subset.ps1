$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

# Encoding-proof: the CJK glyphs and × are expressed as ASCII codepoints (U+xxxx),
# never as literal Chinese inside this file. Windows PowerShell 5.1 reads a no-BOM
# .ps1 using the system ANSI codepage, so literal 中文 passed through --text gets
# mojibaked and those glyphs are silently dropped from the subset (=> tofu in the
# info panel). Keep the character set as U+ codes to avoid that entirely.
#
# Coverage: the ASCII the info panel / size label draw (digits, the specific
# upper/lower-case letters, space, # ( ) , % -) + × (U+00D7) + the shortcut CJK
# 色 值 格 式 复 制 坐 标 模. Adding a NEW CJK glyph means appending its U+ code here.
$unicodes = "U+0020,U+0023,U+0025,U+0028,U+0029,U+002C,U+002D,U+0030-0039,U+0041-0048,U+004C,U+0050,U+0052-0053,U+0058-0059,U+0061-0068,U+0069,U+006C,U+0070,U+0072-0074,U+0078-0079,U+00D7,U+8272,U+503C,U+683C,U+5F0F,U+590D,U+5236,U+5750,U+6807,U+6A21"

New-Item -ItemType Directory -Force -Path subfont | Out-Null
$output = "subfont/harmonyos-sans-sc-subset.ttf"

python3 -m fontTools.subset "refer/HarmonyOS_SansSC_Regular.ttf" `
    --unicodes="$unicodes" `
    --output-file="$output" `
    --layout-features="" `
    --no-hinting `
    --name-IDs="1,2" `
    --name-languages="0x409" `
    --drop-tables+=DSIG,hdmx,VDMX,LTSH,PCLT,gasp,meta `
    --notdef-glyph `
    --recommended-glyphs

# Ship it into the crate asset that include_bytes! embeds:
#   Copy-Item subfont/harmonyos-sans-sc-subset.ttf src-tauri/fonts/harmonyos-sans-sc-subset.ttf -Force
$sizeKB = [math]::Round((Get-Item $output).Length / 1KB, 1)
Write-Output "wrote $output  ($sizeKB KB)"
