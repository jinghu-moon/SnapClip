"""Verify FP16 row source -> PQ grid -> independent AV1 -> absolute scRGB.

Only tiny, self-generated fixtures are materialized. No desktop, clipboard or
display settings are read. This is CPU qualification, not an HDR monitor test.
"""
import argparse
import importlib
import json
import math
from pathlib import Path
import struct
import subprocess

grid = importlib.import_module("verify-hdr-avif-grid")

# Independently rounded D65 Rec.709/Rec.2020 primary transforms. Do not call the
# Rust conversion under test to obtain the reference PQ or reconstructed scRGB.
TO_2020 = ((0.627404, 0.329283, 0.043313),
           (0.069097, 0.919540, 0.011362),
           (0.016391, 0.088013, 0.895595))
TO_709 = ((1.660491, -0.587641, -0.072850),
          (-0.124550, 1.132900, -0.008349),
          (-0.018151, -0.100579, 1.118730))


def pq(nits):
    power = (max(0., min(10000., nits)) / 10000.) ** (2610. / 16384.)
    return ((3424. / 4096. + 2413. / 128. * power) /
            (1. + 2392. / 128. * power)) ** (2523. / 32.)


def nits(code):
    power = max(0., min(1., code)) ** (32. / 2523.)
    return 10000. * (max(0., power - 3424. / 4096.) /
                     (2413. / 128. - 2392. / 128. * power)) ** (16384. / 2610.)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--output", type=Path, default=Path("target/hdr-avif-band-fixtures"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(args.probe.resolve()), "--band-fixture-dir", str(args.output.resolve())], check=True)
    original = list(struct.iter_unpack("<eeee", (args.output / "source-rgba16f-le.bin").read_bytes()))
    if len(original) != 133 * 257 or not any(min(p[:3]) < 0 for p in original) or not any(max(p[:3]) > 1 for p in original):
        raise RuntimeError("FP16 fixture lost its complete source, negative gamut or highlights")
    expected = grid.words(args.output / "expected-rgba64le.bin")
    for name in ("bands-pq", "master-pq"):
        source = args.output / f"{name}.avif"
        result = subprocess.run([args.ffprobe, "-v", "error", "-show_stream_groups", "-of", "json", str(source)],
                                check=True, capture_output=True, text=True)
        decoded = grid.decode_groups(source, json.loads(result.stdout)["stream_groups"], args.ffmpeg, args.output, (133, 257, 6))
        candidate_path = args.output / f"{name}-candidate.bin"
        result = subprocess.run([str(args.probe.resolve()), "--decode", str(source.resolve()), str(candidate_path.resolve())],
                                check=True, capture_output=True, text=True)
        info = json.loads(result.stdout)
        if (info["width"], info["height"], info["depth"], info["alpha"]) != (133, 257, 10, True):
            raise RuntimeError("row-source candidate metadata changed")
        if grid.words(candidate_path) != decoded:
            raise RuntimeError("candidate and independent grid decode disagree")
        if len(decoded) != len(expected) or len(decoded) != len(original) * 4:
            raise RuntimeError("row-source grid is incomplete")
        max_code_error = max_alpha_error = max_source_error = 0.
        negative_preserved = False
        for ix, rgba in enumerate(original):
            if not all(math.isfinite(v) for v in rgba):
                raise RuntimeError("non-finite source fixture")
            reference = [pq(sum(a * b for a, b in zip(row, rgba[:3])) * 80.) for row in TO_2020]
            values = decoded[ix * 4:ix * 4 + 4]
            for channel in range(3):
                # Keep the existing two native ten-bit-code error gate. The
                # source is FP16 -> PQ16 -> PQ10, not a lossless FP16 roundtrip.
                max_code_error = max(max_code_error, abs(values[channel] / 65535. - reference[channel]) * 1023.)
                if abs(expected[ix * 4 + channel] / 65535. - reference[channel]) * 1023. > .02:
                    raise RuntimeError("Rust band conversion disagrees with independent source-color reference")
            max_alpha_error = max(max_alpha_error, abs(values[3] / 65535. - rgba[3]) * 1023.)
            linear_2020 = [nits(value / 65535.) / 80. for value in values[:3]]
            bounds = [max(abs(nits(code + 2. / 1023.) - nits(code)),
                          abs(nits(code - 2. / 1023.) - nits(code))) / 80. for code in reference]
            reconstructed = [sum(a * b for a, b in zip(row, linear_2020)) for row in TO_709]
            for row, actual, wanted in zip(TO_709, reconstructed, rgba[:3]):
                error = abs(actual - wanted)
                max_source_error = max(max_source_error, error)
                # Propagate the unchanged PQ-code envelope through EOTF and the
                # gamut matrix. Rounded reference matrices add <=0.001 scRGB.
                if error > sum(abs(a) * bound for a, bound in zip(row, bounds)) + .001:
                    raise RuntimeError("reconstructed absolute scRGB exceeds the two-code envelope")
            if rgba[0] < -.1 and reconstructed[0] < -.1:
                negative_preserved = True
        if max(max_code_error, max_alpha_error) > 2. or not negative_preserved:
            raise RuntimeError("row source exceeded unchanged pixel gate or clipped negative scRGB")
        if grid.associated_colors(source.read_bytes()) != [b"nclx\x00\x09\x00\x10\x00\x00\x80"]:
            raise RuntimeError("converted PQ pixels acquired an unrelated ICC or incorrect nclx")
        print(f"{name}: 12 independent streams; source PQ error={max_code_error:.6f}/1023; alpha={max_alpha_error:.6f}/1023; reconstructed scRGB error={max_source_error:.6f}; negative preserved", flush=True)
    if (args.output / "bands-pq.avif").read_bytes() != (args.output / "master-pq.avif").read_bytes():
        raise RuntimeError("band and durable-master paths differ")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
