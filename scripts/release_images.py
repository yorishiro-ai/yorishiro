#!/usr/bin/env python3
"""Carry immutable image identities from Bake through smoke and promotion."""

import argparse
import json
import re
import subprocess
from pathlib import Path


def digest(value):
    if not isinstance(value, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", value):
        raise ValueError("missing or invalid image digest")
    return value


def load(path, args, edition, arch=None):
    record = json.loads(path.read_text())
    for key, expected in (("image", args.image), ("source", args.source), ("run_id", args.run_id), ("edition", edition), ("arch", arch)):
        if record.get(key) != expected:
            raise ValueError(f"{path}: wrong {key}")
    digest(record.get("digest"))
    return record


def write(path, record):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, sort_keys=True) + "\n")


def docker(*args):
    return subprocess.check_output(["docker", "buildx", "imagetools", *args], text=True, timeout=120)


def record_build(args):
    metadata = json.loads(args.metadata.read_text())
    records = []
    for edition in ("ce", "ee"):
        records.append({"image": args.image, "source": args.source, "run_id": args.run_id, "edition": edition,
                        "arch": args.arch, "digest": digest(metadata[f"release-{edition}"]["containerimage.digest"])})
    for record in records:
        write(args.directory / f"{record['edition']}-{args.arch}.json", record)


def join(args):
    records = [load(args.directory / f"{args.edition}-{arch}.json", args, args.edition, arch) for arch in ("amd64", "arm64")]
    candidate = f"{args.image}:candidate-{args.run_id}-{args.edition}"
    docker("create", "--annotation", "index:org.opencontainers.image.description=An MCP-native, multi-tenant knowledge store with user-defined schemas.",
           "-t", candidate, *(f"{args.image}@{record['digest']}" for record in records))
    actual = json.loads(docker("inspect", candidate, "--format", "{{json .Manifest}}"))
    record = {"image": args.image, "source": args.source, "run_id": args.run_id, "edition": args.edition,
              "digest": digest(actual.get("digest")), "architectures": {row["arch"]: row["digest"] for row in records}}
    write(args.directory / f"manifest-{args.edition}.json", record)


def reference(args):
    record = load(args.directory / f"manifest-{args.edition}.json", args, args.edition)
    return f"{args.image}@{record['digest']}"


def promote(args):
    records = []
    for edition in ("ce", "ee"):
        record = load(args.directory / f"manifest-{edition}.json", args, edition)
        smoke = load(args.directory / f"smoke-{edition}.json", args, edition)
        if smoke != record:
            raise ValueError("smoke did not verify the exact manifest")
        architectures = record.get("architectures", {})
        if set(architectures) != {"amd64", "arm64"}:
            raise ValueError("incomplete architecture provenance")
        for value in architectures.values():
            digest(value)
        records.append(record)
    for record in records:
        target = f"{args.image}:{args.tag}-{record['edition']}"
        docker("create", "-t", target, f"{args.image}@{record['digest']}")
        actual = json.loads(docker("inspect", target, "--format", "{{json .Manifest}}"))
        if actual.get("digest") != record["digest"]:
            raise ValueError("promoted manifest changed identity")
        for arch, value in record["architectures"].items():
            docker("create", "--prefer-index=false", "-t", f"{target}-{arch}", f"{args.image}@{value}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("record", "join", "reference", "smoked", "promote"))
    parser.add_argument("--image", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--edition", choices=("ce", "ee"))
    parser.add_argument("--arch", choices=("amd64", "arm64"))
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--tag")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.source) or not args.run_id.isdigit():
        parser.error("expected prepared commit and workflow run ID")
    if args.command == "record":
        if not args.metadata or not args.arch:
            parser.error("record requires metadata and architecture")
        record_build(args)
    elif args.command == "join":
        if not args.edition:
            parser.error("join requires edition")
        join(args)
    elif args.command in ("reference", "smoked"):
        if not args.edition:
            parser.error("reference/smoked requires edition")
        if args.command == "reference":
            print(reference(args))
        else:
            record = load(args.directory / f"manifest-{args.edition}.json", args, args.edition)
            write(args.directory / f"smoke-{args.edition}.json", record)
    else:
        if not args.tag:
            parser.error("promote requires release tag")
        promote(args)


if __name__ == "__main__":
    main()
