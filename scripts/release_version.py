#!/usr/bin/env python3
"""Prepare stable Cargo versions and compute package versions for previews."""

import re
import sys
import tomllib
from pathlib import Path


def stable_parts(version):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("stable version must be X.Y.Z without leading zeros")
    return tuple(map(int, version.split(".")))


def prerelease_package_version(current, tag):
    match = re.fullmatch(r"pre-([0-9]{8})-([1-9][0-9]*)", tag)
    if not match:
        raise ValueError("prerelease tag must be pre-YYYYMMDD-N")
    major, minor, patch = stable_parts(current)
    date, sequence = match.groups()
    return f"{major}.{minor}.{patch + 1}~pre.{date}.{sequence}"


def set_stable_version(manifest, current, version):
    if stable_parts(version) < stable_parts(current):
        raise ValueError(f"release version {version} precedes {current}")
    lock = Path("Cargo.lock")
    old = f'name = "yorishiro"\nversion = "{current}"'
    new = f'name = "yorishiro"\nversion = "{version}"'
    sources = {path: path.read_text() for path in (manifest, lock)}
    for path, source in sources.items():
        if source.count(old) != 1:
            raise ValueError(f"{path}: expected one root package entry matching Cargo.toml")
    for path, source in sources.items():
        if current != version:
            path.write_text(source.replace(old, new))


if __name__ == "__main__":
    manifest = Path("Cargo.toml")
    current = tomllib.loads(manifest.read_text())["package"]["version"]
    if sys.argv[1] == "--set-stable":
        set_stable_version(manifest, current, sys.argv[2])
    else:
        print(prerelease_package_version(current, sys.argv[1]))
