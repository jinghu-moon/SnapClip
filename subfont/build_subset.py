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

So the character set is **scanned out of the Rust sources** (`rust_literals.py`): every non-ASCII
character inside a string or char literal, plus printable ASCII as a blanket (see below). Nothing
has to be remembered by whoever adds UI text, and nothing lives in a `.ps1` for PowerShell 5.1 to
mojibake through the system ANSI codepage (which is how the old hand-written `U+xxxx` list lost
glyphs silently).

Over-inclusion is deliberate and cheap: a literal that is only used in a test or a panic message
costs a few hundred bytes, while an *under*-inclusion is a string that silently renders in a
fallback font. The other direction — text assembled at runtime that no scan can see — is covered by
the gate in `win::d2d::tests::the_embedded_subset_covers_the_strings_the_overlay_draws`, which asks
the drawing code for its strings and fails when the embedded subset cannot render one.
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.subset import main as subset_main

from rust_literals import literals_in

ROOT = Path(__file__).resolve().parent.parent
SCAN_ROOT = ROOT / "src-tauri" / "src"
SOURCE = ROOT / "refer" / "HarmonyOS_SansSC_Regular.ttf"
BUILT = ROOT / "subfont" / "harmonyos-sans-sc-subset.ttf"
# The exact path `src-tauri/src/platform/windows/capture/win/d2d.rs` embeds with include_bytes!.
INSTALLED = ROOT / "src-tauri" / "fonts" / "harmonyos-sans-sc-subset.ttf"

# Tables the renderer never reads; dropping them keeps the blob small (see the original script).
DROP_TABLES = (
    "DSIG,hdmx,VDMX,LTSH,PCLT,gasp,meta,kern,GPOS,GSUB,GDEF,BASE,JSTF,MATH,prep,fpgm,cvt"
)


def required_codepoints() -> set[int]:
    """Printable ASCII, plus every non-ASCII character inside a Rust literal."""
    # The Latin side is taken whole. The panel draws *computed* text — colour values, coordinates,
    # sizes, percentages — so no scan and no list can enumerate the digits and hex letters that will
    # appear: the first version of this pipeline used an example-derived list and was missing `3`.
    # 95 glyphs cost ~4 KB and remove the entire class of bug.
    codes = set(range(0x20, 0x7F))
    files = 0
    found = 0
    for path in sorted(SCAN_ROOT.rglob("*.rs")):
        files += 1
        for literal in literals_in(path):
            for char in literal:
                if ord(char) > 0x7F:
                    found += 1
                    codes.add(ord(char))
    print(f"scanned:  {files} Rust files under {SCAN_ROOT.relative_to(ROOT)}, "
          f"{found} non-ASCII characters inside literals")
    if found == 0:
        raise SystemExit("the scan found no non-ASCII literals — is the source root right?")
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
    print(f"required: {len(required)} codepoints")

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
