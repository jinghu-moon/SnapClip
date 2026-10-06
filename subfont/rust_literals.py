"""Pull the string literals out of Rust source, so the font subset can be derived from the code.

Only literals are returned: `//` and (nested) `/* */` comments are skipped, and so is everything
that is not inside quotes. That matters here because this repository's comments are mostly Chinese
prose — scanning raw text would drag the whole design vocabulary into a font that only has to
carry what the overlay actually paints.

Handles the literal forms that appear in Rust: `"…"` with escapes, `b"…"`, `r"…"`, `r#"…"#`
(any number of `#`), `br#"…"#`, and character literals, which are kept apart from lifetimes
(`'a` needs no closing quote, `'a'` does).

Not handled, on purpose: macros that *assemble* text from elsewhere (`include_str!`, localized
resources, `format!` arguments that are runtime data). Those are covered from the other side by
the coverage gate in `win::d2d::tests`, which asks the drawing code for its strings and fails the
build when the embedded subset cannot render one of them.
"""

from __future__ import annotations

from pathlib import Path
from typing import Iterator


def rust_literals(source: str) -> Iterator[str]:
    """Every string / char literal in `source`, in order, comments excluded."""
    index = 0
    length = len(source)
    while index < length:
        if source.startswith("//", index):
            newline = source.find("\n", index)
            index = length if newline < 0 else newline + 1
            continue
        if source.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth > 0:
                if source.startswith("/*", index):
                    depth += 1
                    index += 2
                elif source.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            continue

        # Raw strings: r"…", r#"…"#, br#"…"#. No escapes inside, so the terminator is exact.
        if source[index] == "r" or source.startswith("br", index):
            at = index + 1 if source[index] == "r" else index + 2
            hashes = 0
            while at < length and source[at] == "#":
                hashes += 1
                at += 1
            if at < length and source[at] == '"':
                terminator = '"' + "#" * hashes
                end = source.find(terminator, at + 1)
                if end < 0:
                    return
                yield source[at + 1 : end]
                index = end + len(terminator)
                continue

        # Byte strings: the `b` is noise, the quotes are handled below.
        if source.startswith('b"', index):
            index += 1
            continue

        if source[index] == '"':
            literal: list[str] = []
            at = index + 1
            while at < length:
                char = source[at]
                if char == "\\":
                    # Keep the escaped character itself: `\"` contributes `"`, `\u{4E2D}` a `中`
                    # once written — either way the glyph is the drawn one.
                    literal.append(source[at + 1 : at + 2])
                    at += 2
                    continue
                if char == '"':
                    break
                literal.append(char)
                at += 1
            yield "".join(literal)
            index = at + 1
            continue

        # Character literal vs lifetime: `'a'` closes, `'a` does not.
        if source[index] == "'":
            if index + 2 < length and source[index + 1] != "\\" and source[index + 2] == "'":
                yield source[index + 1]
                index += 3
                continue
            if index + 2 < length and source[index + 1] == "\\":
                end = source.find("'", index + 2)
                if end > 0:
                    yield source[index + 2 : end]
                    index = end + 1
                    continue
            index += 1
            continue

        index += 1


def literals_in(path: Path) -> Iterator[str]:
    yield from rust_literals(path.read_text(encoding="utf-8"))


def literals_with_lines(source: str) -> Iterator[tuple[int, str]]:
    """(`1-based line`, literal) for every string / char literal, for error messages."""
    line = 1
    consumed = 0
    # `rust_literals` loses position, so walk the source in slices: each literal's line is the
    # number of newlines before its first character.
    for literal in rust_literals(source):
        at = source.find(literal, consumed) if literal else -1
        if at < 0:
            yield (line, literal)
            continue
        line += source.count("\n", consumed, at)
        consumed = at + len(literal)
        yield (line, literal)


def main() -> int:
    """Diagnostic: what would the subset pick up, and from where."""
    import sys

    root = Path(__file__).resolve().parent.parent / "src-tauri" / "src"
    seen: dict[str, set[str]] = {}
    files = 0
    for path in sorted(root.rglob("*.rs")):
        files += 1
        for literal in literals_in(path):
            for char in literal:
                if ord(char) > 0x7F:
                    seen.setdefault(char, set()).add(str(path.relative_to(root)))
    print(f"scanned {files} Rust files under {root}")
    print(f"non-ASCII characters inside literals: {len(seen)}")
    for char in sorted(seen):
        where = sorted(seen[char])
        print(f"  U+{ord(char):04X} {char!r:<6} {where[0]}" + (f" (+{len(where) - 1})" if len(where) > 1 else ""))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
