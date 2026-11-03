"""Audit the locked CPU codec closure and optionally assemble its source notices.

This records package declarations and original notices, not a legal opinion or
an assertion that the whole application's release obligations are satisfied.
Run --update after a reviewed lock/feature change; CI uses the default check.
--bundle copies fixed source archives, local snapshots and reviewed patches.
The complete SnapClip source release is still required by the release process.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / "tools/codec-bench/hdr-avif-licenses.json"

def sha(data):
    return hashlib.sha256(data).hexdigest()

def local_text(path):
    # Git's normal Windows checkout may use CRLF. Audit local UTF-8 text in
    # canonical LF form, and put those same bytes into the source bundle.
    # Original upstream archives, patches and notices retain exact byte hashes.
    return path.read_text(encoding="utf8").replace("\r\n", "\n").encode("utf8")

def inventory(config, bundle):
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", "x86_64-pc-windows-msvc", "--features", "snapclip-app/avif"]
    if config:
        command += ["--config", config]
    metadata = json.loads(subprocess.run(command, cwd=ROOT, capture_output=True, check=True).stdout)
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {p["id"]: p for p in metadata["resolve"]["nodes"]}
    root = next(p["id"] for p in packages.values() if p["name"] == "snapclip-avif")
    todo, closure = [root], set()
    while todo:
        current = todo.pop()
        if current in closure:
            continue
        closure.add(current)
        todo.extend(dep["pkg"] for dep in nodes[current]["deps"] if any(k["kind"] in (None, "build") for k in dep["dep_kinds"]))
    locked = tomllib.loads((ROOT / "Cargo.lock").read_text())
    checksums = {(p["name"], p["version"]): p.get("checksum") for p in locked["package"]}
    special = {p["name"]: p for p in json.loads((ROOT / "tools/codec-bench/hdr-avif-sources.json").read_text())}
    cache = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "registry/cache"
    supplemental = json.loads((ROOT / "tools/codec-bench/license-texts/sources.json").read_text(encoding="utf8"))
    result = []
    for ident in sorted(closure, key=lambda i: (packages[i]["name"], packages[i]["version"])):
        package = packages[ident]
        name, version = package["name"], package["version"]
        if not package["license"]:
            raise RuntimeError(f"{name}: missing license declaration")
        folder = Path(package["manifest_path"]).parent
        files = sorted(p for p in folder.iterdir() if p.is_file() and p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")))
        if package.get("license_file"):
            files = sorted(set(files) | {Path(package["license_file"])})
        local = package["source"] is None and name not in special
        evidence = [p for p in supplemental if (p["name"],p["version"]) == (name,version)]
        if not files:
            for item in evidence:
                if "file" in item:
                    path = ROOT / item["file"]
                    if sha(path.read_bytes()) != item["sha256"]:
                        raise RuntimeError(f"{name}: pinned upstream notice changed")
                    files.append(path)
        if local and not files:
            # Workspace license text belongs to the repository; do not infer
            # another package's text merely from its SPDX expression.
            repository = ROOT if name.startswith("snapclip-") else ROOT.parent / "GPUI-Yue-Component"
            files = [repository / "LICENSE"]
        missing = any(p.get("missing_original_text") for p in evidence)
        if not files and not missing:
            raise RuntimeError(f"{name} {version}: original license/notice file missing")
        notices = {p.name: sha(local_text(p) if local else p.read_bytes()) for p in files}
        record = dict(name=name, version=version, license=package["license"], features=sorted(nodes[ident]["features"]), notices=notices)
        if evidence:
            record["supplemental_sources"] = evidence
        if missing:
            record["release_review_required"] = "Original package and pinned upstream tree declare MIT but contain no original license text; obtain upstream clarification before release."
        destination = None
        if bundle:
            destination = bundle / "packages" / f"{name}-{version}"
            destination.mkdir(parents=True, exist_ok=True)
            for path in files:
                (destination / path.name).write_bytes(local_text(path) if local else path.read_bytes())
        if local:
            sources = [folder / "Cargo.toml", *sorted((folder / "src").rglob("*"))]
            if (folder / "build.rs").is_file():
                sources.append(folder / "build.rs")
            record["source_files"] = {p.relative_to(folder).as_posix(): sha(local_text(p)) for p in sources if p.is_file()}
            if destination:
                for path in sources:
                    if path.is_file():
                        target = destination / path.relative_to(folder)
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(local_text(path))
        else:
            checksum = special.get(name, {}).get("checksum") or checksums[(name, version)]
            archive = next(cache.glob(f"*/{name}-{version}.crate"), None)
            if not archive or sha(archive.read_bytes()) != checksum:
                raise RuntimeError(f"{name} {version}: absent or changed original source archive")
            record["source_archive_sha256"] = checksum
            if name in special and "vendor" in special[name]:
                patch = ROOT / special[name]["vendor"] / "SNAPCLIP.patch"
                record["patch_sha256"] = sha(patch.read_bytes())
                if destination:
                    shutil.copyfile(patch, destination / "SNAPCLIP.patch")
            if destination:
                shutil.copyfile(archive, destination / archive.name)
        result.append(record)
    return dict(schema=2, scope="snapclip-avif normal/build closure with snapclip-app/avif", local_text_line_endings="LF", packages=result)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--bundle", type=Path)
    args = parser.parse_args()
    result = inventory(args.config, args.bundle)
    encoded = json.dumps(result, ensure_ascii=False, indent=2) + "\n"
    if args.update:
        INVENTORY.write_text(encoded, encoding="utf8")
    elif INVENTORY.read_text(encoding="utf8") != encoded:
        raise RuntimeError("codec source/license/feature inventory changed; inspect before --update")
    if args.bundle:
        args.bundle.mkdir(parents=True, exist_ok=True)
        (args.bundle / "inventory.json").write_text(encoded, encoding="utf8")
        shutil.copyfile(ROOT / "LICENSE", args.bundle / "SNAPCLIP-LICENSE-AGPL3")
        notices = ["SnapClip optional HDR AVIF CPU codec", "Source declarations and original texts; this is the codec closure, not the complete application release.", "SnapClip selects the AGPL-3.0-only option where AGPL/commercial dual licensing is declared.", ""]
        notices += [f"{p['name']} {p['version']} — {p['license']}" for p in result["packages"]]
        (args.bundle / "THIRD-PARTY-NOTICES.txt").write_text("\n".join(notices) + "\n", encoding="utf8")
        spdx_packages=[]
        for package in result['packages']:
            ident='SPDXRef-'+package['name'].replace('_','-')+'-'+package['version']
            item=dict(SPDXID=ident,name=package['name'],versionInfo=package['version'],
                      downloadLocation=f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download" if 'source_archive_sha256' in package else 'NOASSERTION',
                      filesAnalyzed=False,licenseDeclared=package['license'],
                      licenseConcluded='AGPL-3.0-only' if package['license'].startswith('AGPL-3.0-only') else 'NOASSERTION',
                      copyrightText='NOASSERTION')
            if 'source_archive_sha256' in package:
                item['checksums']=[dict(algorithm='SHA256',checksumValue=package['source_archive_sha256'])]
            spdx_packages.append(item)
        document=dict(spdxVersion='SPDX-2.3',dataLicense='CC0-1.0',SPDXID='SPDXRef-DOCUMENT',
                      name='SnapClip HDR AVIF CPU codec closure',documentNamespace='https://snapclip.local/spdx/hdr-avif/'+sha(encoded.encode()),
                      creationInfo=dict(creators=['Tool: check-hdr-codec-licenses.py'],created='2026-11-04T00:00:00Z'),packages=spdx_packages,
                      relationships=[dict(spdxElementId='SPDXRef-DOCUMENT',relationshipType='DESCRIBES',relatedSpdxElement=p['SPDXID']) for p in spdx_packages],
                      hasExtractedLicensingInfos=[dict(licenseId='LicenseRef-Imazen-Commercial',name='Imazen commercial alternative (not selected)',extractedText=(ROOT/'vendor/zenavif/LICENSE-COMMERCIAL').read_text(encoding='utf8'))])
        (args.bundle/'codec.spdx.json').write_text(json.dumps(document,indent=2)+'\n',encoding='utf8')
    print(f"codec license/source inventory: {len(result['packages'])} locked packages verified")
    unresolved=[p['name'] for p in result['packages'] if 'release_review_required' in p]
    if unresolved:
        print("RELEASE REVIEW REQUIRED: missing original notice text for " + ", ".join(unresolved))
        if args.bundle:
            (args.bundle / "NOT-RELEASE-READY.txt").write_text("Do not distribute this audit bundle as a complete release. Missing original notices: " + ", ".join(unresolved)+"\n",encoding="utf8")
            raise SystemExit(1)

if __name__ == "__main__":
    main()
