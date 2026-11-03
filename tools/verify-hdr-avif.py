"""Cross-check the synthetic candidate fixture with an independent FFmpeg decoder.

The pinned repository patches must satisfy the original numerical contract.
This command never reads user images or the system clipboard.
"""
import argparse
from array import array
from pathlib import Path
import subprocess
import sys


def words(path: Path) -> array:
    result = array("H")
    result.frombytes(path.read_bytes())
    if sys.byteorder != "little":
        result.byteswap()
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--output", type=Path, default=Path("target/hdr-avif-fixtures"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    probe = subprocess.run(
        [str(args.probe.resolve()), "--fixture-dir", str(args.output.resolve())],
        capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
    )
    print(probe.stdout, end="")
    print(probe.stderr, end="", file=sys.stderr)
    (args.output / "candidate.log").write_text(probe.stdout + probe.stderr, encoding="utf-8")
    version = subprocess.run([args.ffmpeg, "-version"], check=True, capture_output=True, text=True)
    print(version.stdout.splitlines()[0])
    # FFmpeg exposes the color and auxiliary alpha as separate streams. Checking
    # RGB alone would not establish that transparency survived the encoder.
    for name, expected_filename, has_alpha in [
        ("pq10-rgb-alpha", "expected-rgba64le.bin", True),
        ("rgb10-colored", "rgb10-colored-expected.bin", False),
        ("rgba10-colored", "rgba10-colored-expected.bin", True),
    ]:
        color_path = args.output / f"{name}-ffmpeg-rgba64le.bin"
        alpha_path = args.output / f"{name}-ffmpeg-alpha16le.bin"
        streams = [("0:v:0", "rgba64le", color_path)]
        if has_alpha:
            streams.append(("0:v:1", "gray16le", alpha_path))
        for stream, pixel_format, output in streams:
            subprocess.run([
                args.ffmpeg, "-v", "error", "-c:v", "libdav1d", "-i", str(args.output / f"{name}.avif"),
                "-map", stream, "-frames:v", "1", "-pix_fmt", pixel_format,
                "-f", "rawvideo", "-y", str(output),
            ], check=True)
        expected = words(args.output / expected_filename)
        actual = words(color_path)
        alpha = words(alpha_path) if has_alpha else actual[3::4]
        if len(expected) != 32 * 16 * 4 or len(actual) != len(expected) or len(alpha) != 32 * 16:
            raise RuntimeError(f"{name}: independent decode dimensions differ from the fixture")
        rgb_error = max(abs(a - b) * 1023 / 65535 for i, (a, b) in enumerate(zip(expected, actual)) if i % 4 != 3)
        alpha_error = max(abs(a - b) * 1023 / 65535 for a, b in zip(expected[3::4], alpha))
        print(f"{name}: independent FFmpeg max RGB code error={rgb_error:.6f}; max Alpha code error={alpha_error:.6f}")
        if rgb_error > 2 or alpha_error > 2:
            raise RuntimeError(f"{name}: independent decoder exceeds the original 2/1023 budget")
    if probe.returncode:
        print("Candidate qualification FAILED; independent RGB/Alpha passed. Keep AVIF disabled.")
        return probe.returncode
    print("Candidate small-fixture checks passed; this alone does not qualify production AVIF.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
