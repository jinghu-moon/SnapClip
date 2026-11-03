"""Verify the actual shared product decoder after the independent 48-case gate.

No display is used. Expected linear pixels are independently reconstructed from
libdav1d output. The original two native-code error budget is propagated through
the PQ EOTF/matrix, plus only FP16 rounding, never replaced by a looser fixed HDR
epsilon. Reject-only metadata cases must allocate under 2 MiB after input load.
"""
import argparse
from array import array
import json
from pathlib import Path
import struct
import subprocess
import sys

MATRIX = ((1.6604910021084345, -0.5876411387885495, -0.0728498633198849),
          (-0.1245504745215907, 1.13289989712596, -0.0083494226043695),
          (-0.0181507633549053, -0.1005788980080074, 1.1187296613629127))

def light(v):
    x = max(0, min(1, v)) ** (32 / 2523)
    return 125 * (max(x - 3424 / 4096, 0) / (2413 / 128 - 2392 / 128 * x)) ** (16384 / 2610)

def check_pixels(actual, reference, depth):
    if len(actual) != len(reference):
        raise AssertionError("output dimensions differ")
    tolerance = 2 / ((1 << depth) - 1)
    for ix in range(0, len(reference), 4):
        rgb = [v / 65535 for v in reference[ix:ix+3]]
        lo = [light(v - tolerance) for v in rgb]
        hi = [light(v + tolerance) for v in rgb]
        for c, row in enumerate(MATRIX):
            lower = sum(a * (l if a >= 0 else h) for a, l, h in zip(row, lo, hi))
            upper = sum(a * (h if a >= 0 else l) for a, l, h in zip(row, lo, hi))
            # Round envelope endpoints to the same FP16 format, rather than
            # hiding quantization in an arbitrary absolute error threshold.
            lower = struct.unpack("<e", struct.pack("<e", lower))[0]
            upper = struct.unpack("<e", struct.pack("<e", upper))[0]
            if not lower-1e-6 <= actual[ix+c] <= upper+1e-6:
                raise AssertionError(f"pixel {ix//4} channel {c}: {actual[ix+c]} outside [{lower},{upper}]")
        if abs(actual[ix+3] - reference[ix+3]/65535) > tolerance + 1/4096:
            raise AssertionError("alpha must remain coverage")

def box(kind, body):
    return struct.pack(">I4s", len(body)+8, kind) + body

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--matrix", type=Path, default=Path("target/hdr-avif-matrix"))
    parser.add_argument("--output", type=Path, default=Path("target/hdr-avif-product"))
    parser.add_argument("--real-inputs", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    cases = json.loads((args.matrix / "summary.json").read_text())["results"]
    if len(cases) != 48:
        raise AssertionError("require all original 48 independent cases")
    reports = []
    for case in cases:
        directory = args.matrix / case["name"]
        output = args.output / (case["name"] + ".fp16")
        result = subprocess.run([str(args.probe.resolve()), "--product-decode", str(directory / "fixture.avif"), str(output)], capture_output=True, check=True)
        metadata = json.loads(result.stdout)
        if not metadata["hdr"] or (metadata["width"], metadata["height"]) != (37, 19):
            raise AssertionError("wrong absolute encoding or output geometry")
        actual = [v[0] for v in struct.iter_unpack("<e", output.read_bytes())]
        reference = array("H")
        reference.frombytes((directory / "independent.bin").read_bytes())
        if sys.byteorder != "little":
            reference.byteswap()
        depth = int(case["name"].split("-")[1])
        check_pixels(actual, reference, depth)
        reports.append(dict(name=case["name"], **metadata))
    ftyp = box(b"ftyp", b"avif\0\0\0\0avif")
    bad = {
        "meta-byte-limit": ftyp + box(b"meta", bytes(1024*1024+1)),
        "item-limit": ftyp + box(b"meta", bytes(4) + box(b"iinf", b"\1\0\0\0" + struct.pack(">I", 0xffffffff))),
        "dimensions": ftyp + box(b"meta", bytes(4) + box(b"iprp", box(b"ipco", box(b"ispe", bytes(4)+struct.pack(">II", 0xffffffff, 0xffffffff))))),
        "icc-curve-limit": ftyp + box(b"meta", bytes(4) + box(b"iprp", box(b"ipco", box(b"colr", b"prof" + bytes(65537))))),
    }
    for name, data in bad.items():
        path = args.output / (name + ".avif")
        path.write_bytes(data)
        result = subprocess.run([str(args.probe.resolve()), "--product-decode", str(path), str(args.output / "must-not-exist.fp16")], capture_output=True)
        if not result.returncode:
            raise AssertionError(f"hostile {name} was accepted")
        memory = json.loads(result.stdout)
        if memory["extra_heap"] >= 2*1024*1024 or memory["largest_allocation"] >= 2*1024*1024:
            raise AssertionError(f"hostile metadata allocated a frame: {memory}")
        reports.append(dict(name=name, **memory))
    if args.real_inputs:
        for path in sorted(args.real_inputs.glob("*.avif")):
            result = subprocess.run([str(args.probe.resolve()), "--product-decode", str(path), str(args.output / "must-not-exist.fp16")], capture_output=True)
            # The supplied pair is 45MP, above the fixed 24MP interactive tier.
            if not result.returncode:
                raise AssertionError("45MP original must not enter the interactive decoder tier")
            memory = json.loads(result.stdout)
            if memory["extra_heap"] >= 16*1024*1024:
                raise AssertionError("oversized real input allocated pixel storage")
            reports.append(dict(name=path.name, **memory))
    (args.output / "summary.json").write_text(json.dumps(reports, indent=2)+"\n", encoding="utf8")
    print(f"product decoder: {len(cases)} independent color/alpha cases; {len(reports)-len(cases)} bounded refusals passed")

if __name__ == "__main__":
    main()
