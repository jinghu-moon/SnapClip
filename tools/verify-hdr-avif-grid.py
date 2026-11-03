"""Cross-check tiled HDR/Alpha AVIF with FFmpeg's independent grid demuxer.

FFmpeg exposes tile groups, not a magically composited first video stream.
Use its reported canvas and per-stream offsets, decode every color/alpha tile
with libdav1d and reassemble only these tiny generated fixtures.
"""
import argparse
from array import array
import importlib
import json
from pathlib import Path
import subprocess
import sys

bmff = importlib.import_module("verify-hdr-avif-metadata")


def words(path):
    values = array("H")
    values.frombytes(path.read_bytes())
    if sys.byteorder != "little":
        values.byteswap()
    return values


def decode_groups(source, groups, ffmpeg, output, layout=(133, 71, 6)):
    planes = []
    if len(groups) != 2:
        raise RuntimeError("expected separate color and alpha grid groups")
    for group in groups:
        if group["type"] != "Tile Grid" or len(group["components"]) != 1:
            raise RuntimeError("independent demuxer did not recognize the grid")
        component = group["components"][0]
        width, height = component["width"], component["height"]
        if (width, height, component["nb_tiles"]) != layout:
            raise RuntimeError("independent grid layout differs from the fixture")
        streams = {stream["index"]: stream for stream in group["streams"]}
        alpha = all(stream["pix_fmt"] == "gray10le" for stream in streams.values())
        channels, pixel_format = (1, "gray16le") if alpha else (4, "rgba64le")
        pixels = array("H", [0]) * (width * height * channels)
        covered = bytearray(width * height)
        for tile in component["subcomponents"]:
            # AVStreamGroupTileGrid stream_index indexes this group's streams;
            # alpha group member 0 may be file-wide stream 6.
            stream = group["streams"][tile["stream_index"]]
            index = stream["index"]
            if not alpha and (stream["pix_fmt"], stream["color_space"], stream["color_transfer"], stream["color_primaries"]) != ("gbrp10le", "gbr", "smpte2084", "bt2020"):
                raise RuntimeError("independent color metadata changed")
            tile_path = output / f"{source.stem}-stream{index}.bin"
            subprocess.run([ffmpeg, "-v", "error", "-c:v", "libdav1d", "-i", str(source),
                            "-map", f"0:{index}", "-frames:v", "1", "-pix_fmt", pixel_format,
                            "-f", "rawvideo", "-y", str(tile_path)], check=True)
            samples = words(tile_path)
            tile_width, tile_height = stream["width"], stream["height"]
            if len(samples) != tile_width * tile_height * channels:
                raise RuntimeError("independent tile sample count differs")
            left, top = tile["tile_horizontal_offset"], tile["tile_vertical_offset"]
            for y in range(min(tile_height, height - top)):
                for x in range(min(tile_width, width - left)):
                    target = (top + y) * width + left + x
                    if covered[target]:
                        raise RuntimeError("independent tiles overlap")
                    covered[target] = 1
                    start = (y * tile_width + x) * channels
                    pixels[target * channels:(target + 1) * channels] = samples[start:start + channels]
        if not all(covered):
            raise RuntimeError("independent tiles leave holes")
        planes.append((alpha, pixels))
    colors = [pixels for alpha, pixels in planes if not alpha]
    alphas = [pixels for alpha, pixels in planes if alpha]
    if len(colors) != 1 or len(alphas) != 1:
        raise RuntimeError("independent groups do not contain color plus alpha")
    rgba = colors[0]
    rgba[3::4] = alphas[0]
    return rgba


def associated_colors(data):
    meta = bmff.boxes(bmff.one(bmff.boxes(data), b"meta")[4:])
    primary = int.from_bytes(bmff.one(meta, b"pitm")[4:6], "big")
    iprp = bmff.boxes(bmff.one(meta, b"iprp"))
    properties = bmff.boxes(bmff.one(iprp, b"ipco"))
    associations = bmff.one(iprp, b"ipma")
    if associations[:4] != bytes(4):
        raise RuntimeError("unsupported fixture ipma")
    cursor, result = 8, []
    for _ in range(int.from_bytes(associations[4:8], "big")):
        item = int.from_bytes(associations[cursor:cursor + 2], "big")
        count = associations[cursor + 2]
        cursor += 3
        indices = associations[cursor:cursor + count]
        cursor += count
        if item == primary:
            result = [properties[(index & 127) - 1] for index in indices]
    if cursor != len(associations):
        raise RuntimeError("trailing fixture association bytes")
    return [value for kind, value in result if kind == b"colr"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--output", type=Path, default=Path("target/hdr-avif-grid-fixtures"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(args.probe.resolve()), "--grid-fixture-dir", str(args.output.resolve())], check=True)
    expected = words(args.output / "expected-rgba64le.bin")
    for name in ["grid-pq", "grid-icc"]:
        source = args.output / f"{name}.avif"
        result = subprocess.run([args.ffprobe, "-v", "error", "-show_stream_groups", "-of", "json", str(source)],
                                check=True, capture_output=True, text=True)
        groups = json.loads(result.stdout)["stream_groups"]
        independent = decode_groups(source, groups, args.ffmpeg, args.output)
        candidate_path = args.output / f"{name}-candidate.bin"
        result = subprocess.run([str(args.probe.resolve()), "--decode", str(source.resolve()), str(candidate_path.resolve())],
                                check=True, capture_output=True, text=True)
        info = json.loads(result.stdout)
        if (info["width"], info["height"], info["depth"], info["alpha"]) != (133, 71, 10, True):
            raise RuntimeError("candidate grid metadata changed")
        candidate = words(candidate_path)
        if len(candidate) != len(expected) or len(independent) != len(expected):
            raise RuntimeError("full-grid sample count differs")
        rgb_error = max(abs(a - b) * 1023 / 65535 for index, (a, b) in enumerate(zip(expected, independent)) if index % 4 != 3)
        alpha_error = max(abs(a - b) * 1023 / 65535 for a, b in zip(expected[3::4], independent[3::4]))
        candidate_error = max(abs(a - b) * 1023 / 65535 for a, b in zip(expected, candidate))
        agreement_error = max(abs(a - b) * 1023 / 65535 for a, b in zip(independent, candidate))
        colors = associated_colors(source.read_bytes())
        if name == "grid-icc":
            profile = (args.output / "expected.icc").read_bytes()
            if candidate_path.with_suffix(".icc").read_bytes() != profile or b"prof" + profile not in colors or not any(p[:4] == b"nclx" for p in colors):
                raise RuntimeError("grid ICC/nclx did not survive association and decode")
        if max(rgb_error, alpha_error, candidate_error, agreement_error) > 2:
            raise RuntimeError("grid exceeded the unchanged 2/1023 gate")
        print(f"{name}: all 12 streams; independent RGB={rgb_error:.6f}, alpha={alpha_error:.6f}; candidate={candidate_error:.6f}, agreement={agreement_error:.6f}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
