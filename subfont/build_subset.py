"""Build the overlay's embedded UI font subset (entered through `subset.ps1`).

Why this exists
---------------
The overlay's DirectWrite text (size label, magnifier info panel, the level hints) draws with
`HarmonyOS Sans SC` — an 8.5 MB system font reduced here to the ~60 glyphs those strings
actually use, then embedded with `include_bytes!` so no machine needs the font installed.

That reduction is a *hard allowlist*: a character that is not in the subset does not fail
anywhere. DirectWrite falls back per glyph, so the string quietly renders half in HarmonyOS and
half in Microsoft YaHei (or as tofu if the fallback is refused). Nothing in `cargo test` notices;
it is visible only on screen.

So the character set is derived from `drawn-glyphs.txt` — the strings themselves, in UTF-8 —
rather than from a hand-maintained list of `U+xxxx` codes inside a PowerShell file:

* PowerShell 5.1 reads a BOM-less `.ps1` with the system ANSI codepage, so literal Chinese in the
  old script was mojibaked and those glyphs were **silently dropped** from the subset;
* and a hand-written list has to be *remembered* by whoever adds UI text. That already drifted
  here: the file shipped to `src-tauri/fonts/` was an older, smaller subset than the script's own
  list, and neither covered the level-hint text added in docs/21 §5.21.

Add a line to `drawn-glyphs.txt` when the overlay starts drawing a new string; this script fails
if the result cannot cover it.
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.subset import main as subset_main

ROOT = Path(__file__).resolve().parent.parent
GLYPHS = ROOT / "subfont" / "drawn-glyphs.txt"
SOURCE = ROOT / "refer" / "HarmonyOS_SansSC_Regular.ttf"
BUILT = ROOT / "subfont" / "harmonyos-sans-sc-subset.ttf"
# The exact path `src-tauri/src/platform/windows/capture/win/d2d.rs` embeds with include_bytes!.
INSTALLED = ROOT / "src-tauri" / "fonts" / "harmonyos-sans-sc-subset.ttf"

# Tables the renderer never reads; dropping them keeps the blob small (see the original script).
DROP_TABLES = (
    "DSIG,hdmx,VDMX,LTSH,PCLT,gasp,meta,kern,GPOS,GSUB,GDEF,BASE,JSTF,MATH,prep,fpgm,cvt"
)


def required_codepoints() -> set[int]:
    """Printable ASCII, plus every character of the listed non-ASCII strings."""
    # The Latin side is taken whole. The panel draws *computed* text — colour values, coordinates,
    # sizes, percentages — so no example list can enumerate the digits and hex letters that will
    # appear: the first version of `drawn-glyphs.txt` was derived from examples and was missing `3`.
    # 95 glyphs cost ~4 KB and remove the entire class of bug.
    codes = set(range(0x20, 0x7F))
    for line in GLYPHS.read_text(encoding="utf-8").splitlines():
        if line.startswith("//"):
            continue
        codes |= {ord(char) for char in line if char != "\r"}
    if len(codes) <= 0x7F - 0x20:
        raise SystemExit(f"{GLYPHS} has no data lines")
    return codes


def covered_codepoints(path: Path) -> set[int]:
    """Codepoints a font's cmap answers with a real glyph."""
    font = TTFont(path)
    covered: set[int] = set()
    for table in font["cmap"].tables:
        for code, name in table.cmap.items():
            if name and name != ".notdef":
                covered.add(code)
    return covered


def main() -> int:
    if not SOURCE.exists():
        raise SystemExit(f"missing source font: {SOURCE}")
    required = required_codepoints()
    unicodes = ",".join(f"U+{code:04X}" for code in sorted(required))
    print(f"required: {len(required)} codepoints from {GLYPHS.name}")

    subset_main(
        [
            str(SOURCE),
            f"--unicodes={unicodes}",
            f"--output-file={BUILT}",
            "--layout-features=",
            "--no-hinting",
            "--desubroutinize",
            "--name-IDs=1,2",
            "--name-languages=0x409",
            f"--drop-tables+={DROP_TABLES}",
            # Keep .notdef so a missing glyph is visible as tofu rather than invisible.
            "--notdef-glyph",
            "--recommended-glyphs",
        ]
    )

    # The gate: the artifact has to cover what the code draws *now*. Without this the failure
    # mode is a half-HTML-ish string on screen that no test and no log mentions.
    covered = covered_codepoints(BUILT)
    missing = sorted(required - covered)
    if missing:
        listing = " ".join(f"U+{code:04X} {chr(code)!r}" for code in missing)
        raise SystemExit(f"the subset does not cover {len(missing)} required glyph(s): {listing}")
    print(f"built:    {BUILT.relative_to(ROOT)}  {BUILT.stat().st_size / 1024:.1f} KB  "
          f"({len(covered)} codepoints, all required ones covered)")

    INSTALLED.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(BUILT, INSTALLED)
    print(f"installed: {INSTALLED.relative_to(ROOT)}  "
          f"({INSTALLED.stat().st_size / 1024:.1f} KB, read by include_bytes!)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
