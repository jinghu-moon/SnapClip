"""Independent lossless libaom fixtures; verify native 8/10/12-bit AVIF decoding.

Requires FFmpeg with libaom-av1 and libdav1d. No display, user image or clipboard
is involved. Every RGB sample is checked against a mathematical reference AND
the independent decoder. No per-fixture exceptions or relaxed error budgets.
"""
import argparse
from array import array
import hashlib
import json
from pathlib import Path
import subprocess
import sys

WIDTH, HEIGHT = 37, 19  # Deliberately non-aligned dimensions/plane strides.


def run(arguments):
    return subprocess.run([str(v) for v in arguments], check=True, capture_output=True)


def pack(values, depth):
    if depth == 8:
        return bytes(values)
    words = array("H", values)
    if sys.byteorder != "little":
        words.byteswap()
    return words.tobytes()


def rgba_words(path):
    words = array("H")
    words.frombytes(path.read_bytes())
    if sys.byteorder != "little":
        words.byteswap()
    return words


def expand(value, depth):
    if depth == 8:
        return value * 257
    shift = 16 - depth
    return (value << shift) | (value >> (depth - shift))


def fixture(depth, matrix, full):
    maximum = (1 << depth) - 1
    factor = 1 << (depth - 8)
    planes = [[], [], []]
    expected = []
    # Deterministic colors and endpoints with deliberately unequal R/G/B.
    levels = [0, 1, maximum // 8, maximum // 4, maximum // 2, maximum - 1, maximum]
    for y in range(HEIGHT):
        for x in range(WIDTH):
            if matrix == "rgb":
                rgb = [levels[(x + 2 * y + offset) % len(levels)] for offset in [0, 2, 4]]
                if not full:
                    rgb = [16 * factor + round(v * 219 * factor / maximum) for v in rgb]
                for plane, value in zip(planes, [rgb[1], rgb[2], rgb[0]]):
                    plane.append(value)
                if not full:
                    rgb = [min(maximum, max(0, ((v - 16 * factor) * maximum + 219 * factor // 2) // (219 * factor))) for v in rgb]
                expected.extend([*(expand(v, depth) for v in rgb), 65535])
            else:
                # Chroma remains in gamut for most samples; clipping endpoints
                # are intentional and checked too. Constant blocks also avoid
                # conflating this 4:4:4 matrix test with chroma interpolation.
                yf = [0.0, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0][(x // 4 + y // 4) % 7]
                cb = [-0.25, 0.0, 0.25][(x // 4) % 3]
                cr = [-0.25, 0.0, 0.25][(y // 4) % 3]
                ycode = round(yf * (maximum if full else 219 * factor)) + (0 if full else 16 * factor)
                ucode = round(cb * (maximum if full else 224 * factor)) + (1 << (depth - 1))
                vcode = round(cr * (maximum if full else 224 * factor)) + (1 << (depth - 1))
                for plane, value in zip(planes, [ycode, ucode, vcode]):
                    plane.append(value)
                yn = (ycode - (0 if full else 16 * factor)) / (maximum if full else 219 * factor)
                un = (ucode - (1 << (depth - 1))) / (maximum if full else 224 * factor)
                vn = (vcode - (1 << (depth - 1))) / (maximum if full else 224 * factor)
                kr, kb = {"bt709": (0.2126, 0.0722), "smpte170m": (0.299, 0.114), "bt2020nc": (0.2627, 0.0593)}[matrix]
                r = yn + 2 * (1 - kr) * vn
                b = yn + 2 * (1 - kb) * un
                g = yn - 2 * kb * (1 - kb) / (1 - kr - kb) * un - 2 * kr * (1 - kr) / (1 - kr - kb) * vn
                expected.extend([*(round(min(1, max(0, v)) * 65535) for v in [r, g, b]), 65535])
    return b"".join(pack(plane, depth) for plane in planes), expected


def check_case(args, depth, matrix, full, alpha=False):
    name = f"{matrix}-{depth}-{'full' if full else 'limited'}{'-alpha' if alpha else ''}"
    directory = args.output / name
    directory.mkdir(parents=True, exist_ok=True)
    raw, expected = fixture(depth, matrix, full)
    (directory / "input.bin").write_bytes(raw)
    alpha_input = []
    if alpha:
        maximum = (1 << depth) - 1
        # Coverage, not PQ light: include zero, first code, midpoints, opaque.
        levels = [0, 1, maximum // 4, maximum // 2, maximum - 1, maximum]
        alpha_input = [levels[(x + y) % len(levels)] for y in range(HEIGHT) for x in range(WIDTH)]
        (directory / "alpha.bin").write_bytes(pack(alpha_input, depth))
        expected[3::4] = [expand(v, depth) for v in alpha_input]
    # Pass planar storage unchanged. FFmpeg's gbrp encoder wrapper forcibly
    # marks every RGB input full-range, even for explicit limited-range flags.
    # yuv444p has the same three-plane layout; CICP=0 defines those planes as
    # GBR and lets libaom encode the requested range without that override.
    pixel_format = "yuv444p" + (f"{depth}le" if depth != 8 else "")
    avif = directory / "fixture.avif"
    gray_format = "gray" + (f"{depth}le" if depth != 8 else "")
    alpha_arguments = (["-f", "rawvideo", "-pix_fmt", gray_format, "-s", f"{WIDTH}x{HEIGHT}",
                        "-color_range", "pc", "-i", directory / "alpha.bin", "-map", "0:v", "-map", "1:v"] if alpha else [])
    run([args.ffmpeg, "-v", "error", "-f", "rawvideo", "-pix_fmt", pixel_format,
         "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", matrix,
         "-color_range", "pc" if full else "tv",
         "-s", f"{WIDTH}x{HEIGHT}", "-i", directory / "input.bin", *alpha_arguments, "-frames:v", "1",
         "-pix_fmt:v:0", pixel_format, *(["-pix_fmt:v:1", gray_format] if alpha else []),
         "-c:v", "libaom-av1", "-cpu-used", "8", "-threads", "1", "-row-mt", "0",
         "-still-picture", "1", "-aom-params", "lossless=1", "-color_primaries:v:0", "bt2020",
         "-color_trc:v:0", "smpte2084", "-colorspace:v:0", matrix, "-color_range:v:0", "pc" if full else "tv",
         *(["-color_range:v:1", "pc"] if alpha else []),
         "-y", avif])
    metadata = json.loads(run([args.probe.resolve(), "--decode", avif, directory / "candidate.bin"]).stdout)
    if matrix == "rgb":
        # libdav1d returns native GBR planes. zimg assumes RGB is already full
        # range, so explicitly normalize all three GBR components here instead
        # of silently accepting its unchanged limited-range RGB result.
        native_format = "gbrp" + (f"{depth}le" if depth != 8 else "")
        native = run([args.ffmpeg, "-v", "error", "-c:v", "libdav1d", "-i", avif,
                      "-frames:v", "1", "-pix_fmt", native_format, "-f", "rawvideo", "-"]).stdout
        if native != raw:
            raise AssertionError(f"{name}: independent native planes differ from lossless source")
        values = list(native) if depth == 8 else list(array("H", native))
        if depth != 8 and sys.byteorder != "little":
            words = array("H", native)
            words.byteswap()
            values = list(words)
        count = WIDTH * HEIGHT
        independent_values = []
        for index in range(count):
            for plane in [2, 0, 1]:
                value = values[plane * count + index]
                normalized = value / ((1 << depth) - 1) if full else (value - 16 * (1 << (depth - 8))) / (219 * (1 << (depth - 8)))
                independent_values.append(round(min(1, max(0, normalized)) * 65535))
            independent_values.append(65535)
        (directory / "independent.bin").write_bytes(pack(independent_values, 16))
    else:
        independent_yuv(args, avif, directory, matrix, full)
    actual = rgba_words(directory / "candidate.bin")
    independent = rgba_words(directory / "independent.bin")
    if alpha:
        native_alpha = run([args.ffmpeg, "-v", "error", "-c:v", "libdav1d", "-i", avif,
                            "-map", "0:v:1", "-frames:v", "1", "-pix_fmt", gray_format,
                            "-f", "rawvideo", "-"]).stdout
        if native_alpha != pack(alpha_input, depth):
            raise AssertionError(f"{name}: independent auxiliary Alpha differs from coverage source")
        independent[3::4] = array("H", [expand(v, depth) for v in alpha_input])
        (directory / "independent.bin").write_bytes(pack(independent, 16))
    if len(actual) != len(expected) or len(independent) != len(expected):
        raise AssertionError(f"{name}: dimensions differ")
    scale = ((1 << depth) - 1) / 65535
    error = max(abs(a - b) * scale for a, b in zip(actual, expected))
    independent_error = max(abs(a - b) * scale for a, b in zip(independent, expected))
    disagreement = max(abs(a - b) * scale for a, b in zip(actual, independent))
    tags = metadata == {"width": WIDTH, "height": HEIGHT, "depth": depth, "primaries": 9,
                        "transfer": 16, "matrix": {"rgb": 0, "bt709": 1, "smpte170m": 6, "bt2020nc": 9}[matrix],
                        "full": full, "alpha": alpha, "icc_bytes": 0}
    report = dict(name=name, max_code_error=error, independent_error=independent_error,
                  disagreement=disagreement, tags_ok=tags, sha256=hashlib.sha256(avif.read_bytes()).hexdigest())
    (directory / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf8")
    print(json.dumps(report), flush=True)
    if not tags or error > 2 or independent_error > 2 or disagreement > 2:
        raise AssertionError(f"{name}: exceeds unchanged 2/native-code budget or invalid tags")
    return report


def independent_yuv(args, avif, directory, matrix, full):
    z_matrix = {"rgb": "gbr", "bt709": "709", "smpte170m": "170m", "bt2020nc": "2020_ncl"}[matrix]
    run([args.ffmpeg, "-v", "error", "-c:v", "libdav1d", "-i", avif, "-frames:v", "1",
         "-vf", f"zscale=matrixin={z_matrix}:rangein={'full' if full else 'limited'}:matrix=gbr:range=full,format=gbrp16le,format=rgba64le",
         "-pix_fmt", "rgba64le", "-f", "rawvideo", "-y", directory / "independent.bin"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--output", type=Path, default=Path("target/hdr-avif-matrix"))
    args = parser.parse_args()
    print(run([args.ffmpeg, "-version"]).stdout.decode().splitlines()[0], flush=True)
    reports, failures = [], []
    for depth in [8, 10, 12]:
        for matrix in ["rgb", "bt709", "smpte170m", "bt2020nc"]:
            for full in [True, False]:
                for alpha in [False, True]:
                    try:
                        reports.append(check_case(args, depth, matrix, full, alpha))
                    except (AssertionError, subprocess.CalledProcessError) as error:
                        details = error.stderr.decode(errors="replace") if isinstance(error, subprocess.CalledProcessError) else str(error)
                        failures.append(f"{matrix}/{depth}/{full}/alpha={alpha}: {details}")
                        print(failures[-1], file=sys.stderr, flush=True)
    (args.output / "summary.json").write_text(json.dumps(dict(results=reports, failures=failures), indent=2) + "\n", encoding="utf8")
    print(f"{len(reports)} passed; {len(failures)} failed")
    return bool(failures)


if __name__ == "__main__":
    raise SystemExit(main())
