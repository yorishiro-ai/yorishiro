#!/usr/bin/env python3
"""Reproduce and verify the documented Loco bug-fix diff against its pinned crate."""

import argparse
import difflib
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

HERE = Path(__file__).resolve().parent
VERSION = "1.2.0"
SHA256 = "5386baf405736a2620001f4561b00fa4874dae99eac3ccf01713fabc9ef13af4"


def files(root):
    return {p.relative_to(root): p.read_bytes() for p in root.rglob("*") if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--refresh", action="store_true", help="regenerate the reviewable diff after intentional source edits")
    parser.add_argument("--prepare", action="store_true", help="restore ignored source when missing, then verify it")
    args = parser.parse_args()
    if args.prepare and args.refresh:
        parser.error("--prepare and --refresh are mutually exclusive")
    cargo = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    archives = list((cargo / "registry/cache").glob(f"*/loco-rs-{VERSION}.crate"))
    with tempfile.TemporaryDirectory(prefix="loco-patch-") as temporary:
        temporary = Path(temporary)
        archive = archives[0] if archives else temporary / "upstream.crate"
        if not archives:
            with urllib.request.urlopen(f"https://static.crates.io/crates/loco-rs/loco-rs-{VERSION}.crate", timeout=60) as response:
                archive.write_bytes(response.read())
        if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
            raise SystemExit("upstream archive checksum mismatch")
        pristine = temporary / "pristine"
        pristine.mkdir()
        with tarfile.open(archive) as tar:
            for member in tar.getmembers():
                member.name = str(Path(member.name).relative_to(f"loco-rs-{VERSION}"))
                if member.name != ".":
                    tar.extract(member, pristine, filter="data")
        source = HERE / "source"
        if not source.exists() and not args.prepare:
            raise SystemExit("source is missing; run python3 patches/loco-rs/verify.py --prepare")
        before, after = files(pristine), files(source)
        if args.refresh:
            diff = []
            for path in sorted(before.keys() | after.keys()):
                if before.get(path) != after.get(path):
                    diff.extend(difflib.unified_diff(
                        before.get(path, b"").decode().splitlines(keepends=True),
                        after.get(path, b"").decode().splitlines(keepends=True),
                        fromfile=f"a/{path}" if path in before else "/dev/null",
                        tofile=f"b/{path}" if path in after else "/dev/null",
                    ))
            (HERE / "fixes.patch").write_text("".join(diff))
        subprocess.run(["patch", "-p1", "--fuzz=0", "--batch", "-i", str(HERE / "fixes.patch")], cwd=pristine, check=True)
        if args.prepare and not source.exists():
            shutil.copytree(pristine, source)
            after = files(source)
        actual = files(pristine)
        changed = [str(path) for path in actual.keys() | after.keys() if actual.get(path) != after.get(path)]
        if changed:
            raise SystemExit("source differs from the documented patch: " + ", ".join(sorted(changed)))
        print(f"Verified loco-rs {VERSION}: archive checksum, exact patch application and source tree")


if __name__ == "__main__":
    main()
