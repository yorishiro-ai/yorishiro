#!/usr/bin/env python3
"""Keep package prereleases between the current and next patch release."""

import re
import sys
import tomllib
from pathlib import Path


def prerelease_package_version(current, tag):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", current):
        raise ValueError("current package version must be X.Y.Z")
    if not re.fullmatch(r"[0-9]{8}-[1-9][0-9]*", tag):
        raise ValueError("prerelease tag must be YYYYMMDD-N")
    major, minor, patch = map(int, current.split("."))
    return f"{major}.{minor}.{patch + 1}~pre.{tag.replace('-', '.')}"


if __name__ == "__main__":
    current = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    print(prerelease_package_version(current, sys.argv[1]))
