"""Build the save panel's numeric font subset from the bundled Fira Code.

Why this exists
---------------
The size cells show a number and a unit. The mockup gives those `.dim-input` and `.dim-unit`
(`docs/design/save-panel-demo-v3.html:242`, `:244`) a `--num` typeface so that digits line up
between the two cells and a changing value does not shuffle the text sideways. That only works if
the face is monospaced.

Fira Code is 283 KB, which is six times the whole embedded UI subset and forty times what the
handful of glyphs actually needed would take. So it is subsetted the same way the UI font is:
reduced to a hard allowlist, with the name table left carrying one English family entry.

That allowlist is a *hard* contract in both directions. A character outside it does not fail
anywhere visible: DirectWrite falls back per glyph, so the string renders half in Fira Code and
half in whatever the system picks, and only the screen shows it. The coverage gate in
`crates/snapclip-capture/src/windows/win/d2d/tests.rs` checks the built artifact against
[`ALLOWED`] below, so a character that is drawn but not listed is a red test rather than a
half-different typeface.

Run: `python3 subfont/build_number_font.py`
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

from fontTools.subset import main as subset_main
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets" / "font" / "FiraCode-Regular.ttf"
BUILT = ROOT / "subfont" / "fira-code-numbers-subset.ttf"
INSTALLED = ROOT / "crates" / "snapclip-ui" / "assets" / "fira-code-numbers-subset.ttf"

# The family the subset registers under, and the name the ui crate asks for. Kept here as well as
# in Rust because this script decides it: the name table is copied from the source font, so a
# rename would have to happen here, and the two would silently disagree.
FAMILY = "Fira Code"

# Exactly the characters the numeric face is used for, read from the one file the Rust side also
# embeds (`crates/snapclip-ui/src/panel/fonts.rs`). The set used to be spelled out twice, once here
# and once in Rust, which is two rules rather than one: a character added for the panel could be
# missing from the subset and only the coverage gate would notice.
#
# It holds the digits, `px` (`:242`, `:244`), `%` for the percentage mode, `KB` and `.` for the
# estimate row's unit (`:648`), and the letters of `.PNG` / `.JPG` / `.WebP` (`:717`), which the
# format control draws in this face (`:173`, `:182`).
ALLOWED = (ROOT / "subfont" / "number-chars.txt").read_text(encoding="utf-8").strip()

# Tables the renderer never reads (same set the UI subset drops).
DROP_TABLES = (
    "DSIG,hdmx,VDMX,LTSH,PCLT,gasp,meta,kern,GPOS,GSUB,GDEF,BASE,JSTF,MATH,prep,fpgm,cvt"
)


def source_codepoints(font: TTFont) -> set[int]:
    return {code for code, name in font.getBestCmap().items() if name and name != ".notdef"}


def main() -> int:
    if not SOURCE.exists():
        print(f"missing source font: {SOURCE.relative_to(ROOT)}", file=sys.stderr)
        return 1

    source = TTFont(SOURCE)
    available = source_codepoints(source)
    missing = sorted({ord(c) for c in ALLOWED} - available)
    if missing:
        # A character the panel draws that the source font cannot provide would render as a
        # fallback glyph, which is exactly what this font exists to prevent.
        print(
            "source font has no glyph for "
            + ", ".join(f"{chr(code)!r} (U+{code:04X})" for code in missing),
            file=sys.stderr,
        )
        return 1

    args = [
        str(SOURCE),
        f"--text={ALLOWED}",
        f"--drop-tables={DROP_TABLES}",
        "--layout-features=",
        "--name-IDs=1,2",
        "--name-languages=0x409",
        f"--output-file={BUILT}",
    ]
    subset_main(args)

    built = TTFont(BUILT)
    covered = source_codepoints(built)
    uncovered = sorted({ord(c) for c in ALLOWED} - covered)
    if uncovered:
        print(
            "built subset is missing "
            + ", ".join(f"{chr(code)!r} (U+{code:04X})" for code in uncovered),
            file=sys.stderr,
        )
        return 1

    families = {
        record.toUnicode()
        for record in built["name"].names
        if record.nameID == 1
    }
    if families != {FAMILY}:
        # The ui crate asks for one family name; if the source font's name table carried anything
        # else, the registration would succeed and the lookup would still miss.
        print(f"family name is {sorted(families)}, expected ['{FAMILY}']", file=sys.stderr)
        return 1

    INSTALLED.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(BUILT, INSTALLED)
    print(
        f"allowed:   {len(ALLOWED)} characters ({ALLOWED!r})\n"
        f"built:     {BUILT.relative_to(ROOT)}  "
        f"{BUILT.stat().st_size / 1024:.1f} KB\n"
        f"installed: {INSTALLED.relative_to(ROOT)}  "
        f"(read by include_bytes!)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
