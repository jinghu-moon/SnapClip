"""Verify vendored codec sources against pinned official release archives + patches.

Does not edit Cargo's registry or download anything implicitly. Pass --download
to fetch missing fixed-version archives from crates.io and verify their recorded
SHA-256 before reading them. All untouched files must match the release bytes.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import tarfile
import urllib.request


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tools/codec-bench/hdr-avif-sources.json"


def patch_for(upstream, current):
    lines = []
    for name in sorted(upstream):
        if current[name] != upstream[name]:
            lines.extend(difflib.unified_diff(
                upstream[name].decode("utf8").splitlines(keepends=True),
                current[name].decode("utf8").splitlines(keepends=True),
                fromfile=f"a/{name}", tofile=f"b/{name}"))
    return "".join(lines).encode("utf8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--download", action="store_true")
    parser.add_argument("--cache", type=Path, default=Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry/cache")
    args = parser.parse_args()
    for package in json.loads(MANIFEST.read_text(encoding="utf8")):
        if "vendor" not in package:
            continue
        name, version = package["name"], package["version"]
        matches = sorted(args.cache.glob(f"*/{name}-{version}.crate"))
        if matches:
            archive = matches[0]
        elif args.download:
            archive = ROOT / "target/hdr-codec-archives" / f"{name}-{version}.crate"
            archive.parent.mkdir(parents=True, exist_ok=True)
            with urllib.request.urlopen(f"https://crates.io/api/v1/crates/{name}/{version}/download", timeout=60) as response:
                archive.write_bytes(response.read())
        else:
            raise RuntimeError(f"missing {name}-{version}.crate; use --cache or explicitly --download")
        if hashlib.sha256(archive.read_bytes()).hexdigest() != package["checksum"]:
            raise RuntimeError(f"{name}: published archive checksum mismatch")
        folder = ROOT / package["vendor"]
        upstream = {}
        with tarfile.open(archive) as source:
            for item in source.getmembers():
                if not item.isfile():
                    continue
                prefix, path = item.name.split("/", 1)
                if prefix != f"{name}-{version}" or ".." in Path(path).parts:
                    raise RuntimeError("unexpected archive path")
                if path not in package["archive_omissions"]:
                    upstream[path] = source.extractfile(item).read()
        current = {path: (folder / path).read_bytes() for path in upstream}
        actual_files = {str(p.relative_to(folder)).replace("\\", "/") for p in folder.rglob("*") if p.is_file()}
        if actual_files != set(upstream) | {"SNAPCLIP.patch"}:
            raise RuntimeError(f"{name}: unrecorded added/missing source files")
        difference = patch_for(upstream, current)
        if difference != (folder / "SNAPCLIP.patch").read_bytes():
            raise RuntimeError(f"{name}: source changes differ from reviewed SNAPCLIP.patch")
        if hashlib.sha256(difference).hexdigest() != package["patch_checksum"]:
            raise RuntimeError(f"{name}: patch checksum mismatch")
        changed = sorted(path for path in upstream if upstream[path] != current[path])
        if changed != package["patch_files"]:
            raise RuntimeError(f"{name}: unexpected modified files")
        print(f"{name} {version}: archive + {len(upstream)} source files + {len(changed)} patched files verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
