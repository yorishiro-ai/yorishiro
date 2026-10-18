#!/usr/bin/env python3
"""Classify changed paths into CI validation groups."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import PurePosixPath

GROUPS = ("rust", "ee", "shared", "config", "migration", "topology", "packaging", "docker", "release", "deps")
ALL = set(GROUPS)

RUST_SETUP_CONSUMERS = {"rust", "ee", "deps", "packaging", "release"}
PACKAGE_ACTION_CONSUMERS = {"packaging", "docker", "release"}
DOCKER_ACTION_CONSUMERS = {"docker", "release"}


def classify_path(path: str) -> set[str]:
    p = PurePosixPath(path)
    s = str(p)
    groups: set[str] = set()

    if s.startswith((".github/actions/rust-setup/", ".github/actions/sea-orm-cli/")):
        groups |= RUST_SETUP_CONSUMERS | {"shared"}
    elif s.startswith(".github/actions/package-tests/") or s.startswith(".github/actions/build-and-package/"):
        groups |= PACKAGE_ACTION_CONSUMERS
    elif s.startswith(".github/actions/build-docker/") or s.startswith(".github/actions/docker-manifest/") or s.startswith(".github/actions/smoke/"):
        groups |= DOCKER_ACTION_CONSUMERS
    elif s.startswith(".github/actions/"):
        return set(ALL)

    if s.startswith("src/"):
        groups |= {"rust"}
    if s.startswith("ee/"):
        groups |= {"ee"}
    if s.startswith("edition/"):
        groups |= {"shared", "rust", "ee"}
    if s.startswith("migration/"):
        groups |= {"migration", "shared", "rust", "ee"}
    if s.startswith(("config/", "locales/")) or (p.name.endswith((".yaml", ".yml")) and "config" in p.parts):
        groups |= {"config"}
    if s.startswith(("tests/", "scripts/test_topology", "scripts/check_ci_matrix")):
        groups |= {"topology", "rust", "ee"}
    if s.startswith(("scripts/classify_ci_changes.py", "scripts/test_classify_ci_changes.py")):
        groups |= {"shared", "rust", "ee"}
    if s.startswith("packaging/") or s.startswith(".github/actions/package-tests/"):
        groups |= {"packaging"}
    if s.startswith("packaging/"):
        groups |= {"rust", "ee"}
    if s.startswith(("Dockerfile", "docker-compose", "docker/")):
        groups |= {"docker"}
    if s.startswith((".github/actions/build-and-package/", ".github/actions/build-docker/", ".github/actions/docker-manifest/", ".github/actions/smoke/")):
        groups |= {"release"}
    if s in {"Cargo.toml", "Cargo.lock", "migration/Cargo.toml"} or (p.name in {"Cargo.toml", "Cargo.lock"}):
        groups |= {"deps", "rust", "ee"}
    workflow_groups = {
        ".github/workflows/ci.yml": {"shared", "rust", "ee"},
        ".github/workflows/release.yml": {"shared", "rust", "ee", "packaging", "docker", "release"},
        ".github/workflows/release_pre.yml": {"shared", "rust", "ee", "packaging", "docker", "release"},
        ".github/workflows/security.yml": {"deps"},
        ".github/workflows/dep-audit.yml": {"deps"},
        ".github/workflows/coverage.yml": {"rust", "ee"},
        ".github/workflows/codeql.yml": {"rust", "ee"},
        ".github/workflows/windows-weekly.yml": {"rust", "ee"},
        ".github/workflows/auto-label.yml": set(),
        ".github/workflows/cache-cleanup.yml": set(),
        ".github/workflows/stale.yml": set(),
    }
    if s.startswith(".github/workflows/"):
        workflow_selected = workflow_groups.get(s)
        return set(ALL) if workflow_selected is None else workflow_selected
    if s.startswith(".github/") and not groups:
        return set(ALL)
    if not groups:
        return set(ALL)
    return groups


def changed_paths(base: str, head: str) -> list[str]:
    result = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", f"{base}...{head}"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    # --no-renames reports a rename as a deletion and an addition, so both sides classify.
    return [line for line in result.stdout.splitlines() if line]


def classify(paths: list[str], force_full: bool = False) -> dict[str, bool]:
    groups = set(ALL) if force_full else set().union(*(classify_path(p) for p in paths)) if paths else set()
    selected = {group: group in groups for group in GROUPS}
    selected["full"] = force_full or groups == ALL
    package_related = bool(groups & {"packaging", "docker", "release"})
    selected["package_build"] = force_full or package_related
    selected["package_validation"] = force_full or package_related
    selected["docker_validation"] = force_full or bool(groups & {"docker", "release"})
    selected["rust_validation"] = force_full or bool(groups & {"rust", "ee", "shared", "config", "migration", "topology", "deps"})
    selected["security_validation"] = force_full or "deps" in groups
    return selected


def selected_job_succeeded(result: str, selected: bool) -> bool:
    """Only success passes selected jobs; unselected jobs must be skipped."""
    return result == ("success" if selected else "skipped")


def package_build_allowed(package_selected: bool, rust_selected: bool, changes_result: str, ce_result: str, ee_result: str, msrv_result: str) -> bool:
    """Select package builds independently, but require Rust checks when both groups apply."""
    return (
        changes_result == "success"
        and package_selected
        and (not rust_selected or (ce_result == ee_result == msrv_result == "success"))
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--paths", nargs="*")
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--output")
    args = parser.parse_args()
    if args.full:
        paths = []
    elif args.paths is not None:
        paths = args.paths
    elif args.base:
        paths = changed_paths(args.base, args.head)
    else:
        parser.error("--base is required unless --full is set")
    force = args.full or not paths
    selected = classify(paths, force)
    payload = json.dumps({"paths": paths, **selected}, sort_keys=True)
    print(payload)
    if args.output:
        with open(args.output, "a", encoding="utf-8") as f:
            for key, value in selected.items():
                f.write(f"{key}={str(value).lower()}\n")
            f.write(f"groups={','.join(g for g in GROUPS if selected[g])}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
