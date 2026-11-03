"""Build real dual-colr fixtures and check ICC precedence independently of order.

This edits only the generated tiny fixture, never arbitrary product input.
All box sizes, primary property associations and absolute iloc offsets are
rebuilt structurally. Both ICC and nclx remain associated with the primary item.
"""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import unittest


def boxes(data):
    result, offset = [], 0
    while offset < len(data):
        if len(data) - offset < 8:
            raise ValueError("truncated fixture box")
        size, kind = struct.unpack_from(">I4s", data, offset)
        if size < 8 or size > len(data) - offset:
            raise ValueError("unsupported or out-of-bounds fixture box")
        result.append((kind, data[offset + 8:offset + size]))
        offset += size
    return result


def box(kind, payload):
    return struct.pack(">I4s", len(payload) + 8, kind) + payload


def contents(items):
    return b"".join(box(kind, payload) for kind, payload in items)


def one(items, kind):
    matches = [payload for name, payload in items if name == kind]
    if len(matches) != 1:
        raise ValueError(f"expected one {kind!r} fixture box")
    return matches[0]


def associations(payload, primary, property_index, before, icc_index):
    version, flags = payload[0], int.from_bytes(payload[1:4], "big")
    if version not in [0, 1] or flags & ~1:
        raise ValueError("unsupported fixture ipma version/flags")
    id_size, association_size = (2 if version == 0 else 4), (2 if flags & 1 else 1)
    index_mask = (1 << (association_size * 8 - 1)) - 1
    if property_index > index_mask:
        raise ValueError("fixture property index exceeds current ipma field")
    count, cursor, output, changed = int.from_bytes(payload[4:8], "big"), 8, bytearray(payload[:8]), False
    for _ in range(count):
        item_id = int.from_bytes(payload[cursor:cursor + id_size], "big")
        cursor += id_size
        length = payload[cursor]
        cursor += 1
        values = [int.from_bytes(payload[i:i + association_size], "big") for i in range(cursor, cursor + length * association_size, association_size)]
        cursor += length * association_size
        if item_id == primary:
            indices = [v & index_mask for v in values]
            if icc_index not in indices:
                raise ValueError("ICC was not associated with fixture primary")
            values.insert(indices.index(icc_index) if before else len(values), property_index)
            changed = True
        output.extend(item_id.to_bytes(id_size, "big"))
        output.append(len(values))
        for value in values:
            output.extend(value.to_bytes(association_size, "big"))
    if cursor != len(payload) or not changed:
        raise ValueError("invalid or missing primary fixture association")
    return bytes(output)


def relocate_iloc(payload, delta, meta_end):
    data, cursor = bytearray(payload), 6
    version = data[0]
    if version not in [0, 1, 2]:
        raise ValueError("unsupported fixture iloc version")
    offset_size, length_size = data[4] >> 4, data[4] & 15
    base_size = data[5] >> 4
    index_size = (data[5] & 15) if version else 0
    def take(size):
        nonlocal cursor
        if cursor + size > len(data):
            raise ValueError("truncated fixture iloc")
        start = cursor
        cursor += size
        return start, int.from_bytes(data[start:cursor], "big")
    _, count = take(4 if version == 2 else 2)
    for _ in range(count):
        take(4 if version == 2 else 2)
        method = (take(2)[1] & 15) if version else 0
        _, reference = take(2)
        _, base = take(base_size)
        _, extent_count = take(2)
        for _ in range(extent_count):
            take(index_size)
            start, offset = take(offset_size)
            take(length_size)
            if method == 0 and reference == 0 and base + offset >= meta_end:
                data[start:start + offset_size] = (offset + delta).to_bytes(offset_size, "big")
    if cursor != len(data):
        raise ValueError("trailing fixture iloc bytes")
    return bytes(data)


def dual_colr(source, before):
    top = boxes(source)
    old_meta = one(top, b"meta")
    children = boxes(old_meta[4:])
    pitm = one(children, b"pitm")
    primary = int.from_bytes(pitm[4:], "big")
    iprp = boxes(one(children, b"iprp"))
    ipco = boxes(one(iprp, b"ipco"))
    icc = [(i + 1, payload[4:]) for i, (kind, payload) in enumerate(ipco) if kind == b"colr" and payload[:4] == b"prof"]
    if len(icc) != 1:
        raise ValueError("fixture must contain exactly one ICC")
    # Deliberately conflict with AV1's Rec.2020/PQ/Identity/full fields.
    ipco.append((b"colr", b"nclx" + struct.pack(">HHHB", 1, 13, 1, 0)))
    iprp = [(kind, contents(ipco) if kind == b"ipco" else associations(payload, primary, len(ipco), before, icc[0][0]) if kind == b"ipma" else payload) for kind, payload in iprp]
    updated = [(kind, contents(iprp) if kind == b"iprp" else payload) for kind, payload in children]
    new_meta = old_meta[:4] + contents(updated)
    delta = len(new_meta) - len(old_meta)
    meta_offset = 0
    for kind, payload in top:
        meta_offset += len(payload) + 8
        if kind == b"meta":
            break
    updated = [(kind, relocate_iloc(payload, delta, meta_offset) if kind == b"iloc" else payload) for kind, payload in updated]
    new_meta = old_meta[:4] + contents(updated)
    result = contents([(kind, new_meta if kind == b"meta" else payload) for kind, payload in top])
    if len(result) != len(source) + delta:
        raise AssertionError("fixture relocation changed unexpected lengths")
    return result, icc[0][1]


class FixtureTests(unittest.TestCase):
    def test_box_bounds_and_roundtrip(self):
        for bad in [b"bad", b"\0\0\0\x01meta", b"\0\0\0\x10meta", b"\0\0\0\0meta"]:
            with self.assertRaises(ValueError):
                boxes(bad)
        value = [(b"test", b"payload"), (b"next", b"42")]
        self.assertEqual(boxes(contents(value)), value)

    def test_association_preserves_essential_flags_and_other_items(self):
        original = bytes.fromhex("00000000 00000002 0001 02 81 82 0002 01 83")
        self.assertEqual(associations(original, 1, 4, True, 2), bytes.fromhex("00000000 00000002 0001 03 81 04 82 0002 01 83"))
        with self.assertRaises(ValueError):
            associations(original, 3, 4, True, 2)

    def test_relocation_changes_absolute_extents_only(self):
        original = bytes.fromhex("00000000 4400 0001 0001 0000 0001 00000080 00000020")
        expected = bytes.fromhex("00000000 4400 0001 0001 0000 0001 00000090 00000020")
        self.assertEqual(relocate_iloc(original, 16, 100), expected)
        self.assertEqual(relocate_iloc(original, 16, 129), original)
        with self.assertRaises(ValueError):
            relocate_iloc(original[:-1], 16, 100)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--fixture-dir", type=Path, default=Path("target/hdr-avif-fixtures"))
    args = parser.parse_args()
    if not unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(FixtureTests)).wasSuccessful():
        return 1
    source = (args.fixture_dir / "icc-nclx-conflict.avif").read_bytes()
    failed = False
    for before in [True, False]:
        dual, icc = dual_colr(source, before)
        name = "nclx-before-icc" if before else "icc-before-nclx"
        path = args.fixture_dir / f"{name}.avif"
        output = args.fixture_dir / f"{name}.bin"
        path.write_bytes(dual)
        run = subprocess.run([str(args.probe.resolve()), "--decode", str(path), str(output)], capture_output=True, check=True)
        metadata = json.loads(run.stdout)
        preserved = output.with_suffix(".icc")
        passed = (preserved.read_bytes() == icc and metadata["primaries"] == 9
                  and metadata["transfer"] == 16 and metadata["matrix"] == 0 and metadata["full"])
        expected = (args.fixture_dir / "expected-rgba64le.bin").read_bytes()
        # The contradictory container nclx must not change raw Identity sample
        # decoding. ICC is exposed intact, not applied silently by this decoder.
        passed &= output.read_bytes() == expected
        print(f"{name}: ICC+bitstream preserved={passed}; metadata={metadata}")
        failed |= not passed
    return failed


if __name__ == "__main__":
    raise SystemExit(main())
