#!/usr/bin/env python3
"""Join compiled inventories to reviewed execution responsibilities.

Only collect executes binaries, using libtest listing in GitHub Actions after one workspace build.
The source guard freezes reviewed Rust dependencies conservatively, including all test helpers.
It is deliberately not a Rust call-graph proof: any Rust edit requires a fresh source review.
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "tests/ci/assignments.json"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            checksum.update(chunk)
    return checksum.hexdigest()


def load(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(Path(path).read_text(), object_pairs_hook=unique)


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def source_census(root):
    # shortcut: whole-tree fingerprints trade precision for fail-closed helper/alias review until an AST guard exists.
    result = {}
    for directory in ("tests", "src", "ee", "migration"):
        files = sorted((root / directory).rglob("*.rs"))
        data = b"".join(str(p.relative_to(root)).encode() + b"\0" + p.read_bytes() + b"\0" for p in files)
        result[directory] = digest(data)
    for name in ("Cargo.toml", "migration/Cargo.toml", "Cargo.lock", ".cargo/config.toml"):
        result[name] = digest((root / name).read_bytes())
    # The verified patch and its pinned upstream checksum define the ignored Loco source tree.
    for name in ("patches/loco-rs/fixes.patch", "patches/loco-rs/verify.py"):
        result[name] = digest((root / name).read_bytes())
    for directory in ("config", "tests/fixtures"):
        files = sorted(p for p in (root / directory).rglob("*")
                       if p.is_file() and "__pycache__" not in p.parts)
        result[directory] = digest(b"".join(str(p.relative_to(root)).encode() + b"\0" +
                                          p.read_bytes() + b"\0" for p in files))
    return result


def source_functions(root, entry="tests/mod.rs", follow_modules=True):
    """Resolve declaring modules, not compiled test names; Cargo/libtest remains authoritative.

    This small lexer supports the checked-in ordinary and inline modules, not arbitrary Rust macros.
    Unknown macro-generated identities or module syntax fail for an exact reviewed source mapping.
    """
    token = re.compile(r'//[^\n]*|/\*.*?\*/|r(\#*)".*?"\1|"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\\n])\'|[A-Za-z_][A-Za-z_0-9]*|[^\s]', re.DOTALL)
    found = {}
    visited = set()

    def visit(path, module, enterprise=False):
        require(path.is_file(), f"missing module source: {path}")
        key = (str(path), module)
        require(key not in visited, f"duplicate module source: {key}")
        visited.add(key)
        text = path.read_text()
        enterprise = enterprise or bool(re.search(r'^#!\[cfg\(feature = "enterprise"\)\]', text))
        tokens = [(m.group(), m.start(), m.end()) for m in token.finditer(text)
                  if not m.group().startswith(("//", "/*"))]
        stack = []
        boundary = 0
        i = 0
        while i < len(tokens):
            word, start, end = tokens[i]
            if word in ("mod", "fn") and i + 2 < len(tokens):
                name = tokens[i + 1][0]
                if re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", name):
                    inline = tuple(item[0] for item in stack if item is not None)
                    current = module + inline
                    attrs = text[boundary:start]
                    gated = enterprise or any(item[1] for item in stack if item is not None)
                    gated = gated or bool(re.search(r'cfg\s*\(\s*feature\s*=\s*"enterprise"\s*\)', attrs))
                    if word == "fn":
                        identity = "::".join(current + (name,))
                        row = {"source": str(path.relative_to(root)), "module": "::".join(current),
                               "editions": ["ee"] if gated else ["ce", "ee"],
                               "is_test": bool(re.search(r'#\s*\[\s*(?:tokio::)?test\b', attrs))}
                        found[identity] = None if identity in found else row
                    elif tokens[i + 2][0] == ";":
                        if not follow_modules:
                            i += 3
                            boundary = tokens[i - 1][2]
                            continue
                        base = path.parent if path.name == "mod.rs" else path.with_suffix("")
                        candidates = [p for p in (base / f"{name}.rs", base / name / "mod.rs") if p.is_file()]
                        require(len(candidates) == 1, f"ambiguous/missing declaring module: {current + (name,)}")
                        visit(candidates[0], current + (name,), gated)
                    elif tokens[i + 2][0] == "{":
                        stack.append((name, gated))
                        boundary = tokens[i + 2][2]
                        i += 3
                        continue
            if word == "{":
                stack.append(None)
                boundary = end
            elif word == "}":
                require(bool(stack), f"unbalanced source braces: {path}")
                stack.pop()
                boundary = end
            elif word == ";":
                boundary = end
            i += 1
        require(not stack, f"unbalanced source braces: {path}")

    visit(root / entry, ())
    return found


def parse_listing(text):
    names = []
    for line in text.splitlines():
        match = re.fullmatch(r"([^\s]+): test", line)
        require(match is not None, f"malformed libtest listing: {line!r}")
        names.append(match[1])
    require(len(names) == len(set(names)), "duplicate libtest identity")
    return sorted(names)


def artifacts(messages, manifest, root):
    expected = {(t["package"], t["target"], t["kind"]): t for t in manifest["targets"]}
    require(len(expected) == len(manifest["targets"]), "duplicate manifest target")
    by_source = {str((root / t["source"]).resolve()): k for k, t in expected.items()}
    require(len(by_source) == len(expected), "ambiguous target source")
    result = {}
    finished = []
    for line in messages.splitlines():
        message = json.loads(line)
        if message.get("reason") == "build-finished":
            finished.append(message.get("success"))
        if message.get("reason") != "compiler-artifact" or not message.get("profile", {}).get("test"):
            continue
        target = message["target"]
        source = str(Path(target["src_path"]).resolve())
        require(source in by_source, f"unreviewed workspace test target: {source}")
        key = by_source[source]
        require(target["name"] == key[1] and target["kind"] == [key[2]], f"wrong target metadata: {target}")
        require(key not in result, f"duplicate compiler artifact: {key}")
        require(message.get("executable"), f"test artifact has no executable: {key}")
        result[key] = message
    require(finished == [True], "expected one successful Cargo build-finished message")
    require(set(result) == set(expected), f"missing workspace test artifacts: {set(expected) - set(result)}")
    return result


def command(argv):
    return subprocess.run(argv, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def collect(messages, edition, manifest, root=ROOT, run=command):
    require(source_census(root) == manifest["source_guard"], "source guard changed; review Rust sources and helper dependencies")
    compiled = []
    for key, message in sorted(artifacts(messages, manifest, root).items()):
        executable = Path(message["executable"]).resolve()
        features = sorted(message["features"])
        if key[0] == "yorishiro":
            require("community" in features and (("enterprise" in features) == (edition == "ee")), "wrong edition features")
        names = parse_listing(run([str(executable), "--list", "--format", "terse"]))
        ignored = parse_listing(run([str(executable), "--list", "--format", "terse", "--ignored"]))
        require(set(ignored) <= set(names), "ignored identities absent from complete listing")
        compiled.append({"package": key[0], "target": key[1], "kind": key[2],
                         "package_id": message["package_id"], "features": features,
                         "profile": message["profile"], "executable": str(executable),
                         "sha256": file_digest(executable), "tests": names, "ignored": ignored})
    rustc = run(["rustc", "-vV"])
    host = re.search(r"^host: (\S+)$", rustc, re.MULTILINE)
    require(host is not None, "missing rustc host triple")
    require(os.environ.get("CARGO_BUILD_TARGET", host[1]) == host[1], "inventory collection supports native builds only")
    return {"schema": 1, "edition": edition, "revision": run(["git", "rev-parse", "HEAD"]).strip(),
            "rustc": rustc, "target_triple": host[1], "source_guard": manifest["source_guard"],
            "manifest_sha256": digest(json.dumps(manifest, sort_keys=True).encode()), "artifacts": compiled}


def verify(ce, ee, manifest, root=ROOT):
    require(source_census(root) == manifest["source_guard"], "source guard changed; review sources, including indirect backend/provider/licence/resource helpers")
    functions = source_functions(root)
    targets = {(t["package"], t["target"], t["kind"]): t for t in manifest["targets"]}
    require(len(targets) == len(manifest["targets"]), "duplicate manifest target")
    exact = {}
    rows = list(manifest["exceptions"])
    for group in manifest.get("exception_groups", []):
        for name in group["names"]:
            rows.append({key: value for key, value in group.items() if key != "names"} | {"name": name})
    for row in rows:
        key = (row.get("package", "yorishiro"), row.get("target", "mod"), row.get("kind", "test"), row["name"])
        require(key not in exact, f"duplicate exact assignment: {key}")
        require(row["reason"] and row["editions"] and len(set(row["editions"])) == len(row["editions"]), f"invalid exact row: {key}")
        require(set(row["editions"]) <= {"ce", "ee"}, f"invalid editions: {key}")
        exact[key] = row
    common = {}
    for row in manifest["common_modules"]:
        require(row["module"] and row["module"] not in common, "ambiguous common module rule")
        require(any(source and source["is_test"] and source["module"] == row["module"]
                    and source["source"] == row["source"] for source in functions.values()),
                f"stale declaring-module rule: {row['module']}")
        common[row["module"]] = row["source"]
    delta = {}
    for row in manifest["ee_delta"]:
        require(row["name"] not in delta and row["reason"] and row["jobs"], "ambiguous/empty EE delta")
        require(len(row["jobs"]) == len(set(row["jobs"])), "duplicate EE delta jobs")
        delta[row["name"]] = row
    require(ce["revision"] == ee["revision"] and ce["rustc"] == ee["rustc"]
            and ce["target_triple"] == ee["target_triple"], "CE/EE build provenance differs")
    inventories = {}
    artifact_records = []
    for edition, inventory in (("ce", ce), ("ee", ee)):
        require(inventory["schema"] == 1 and inventory["edition"] == edition, "wrong inventory edition/schema")
        require(inventory["source_guard"] == manifest["source_guard"], "inventory source guard differs")
        require(inventory["manifest_sha256"] == digest(json.dumps(manifest, sort_keys=True).encode()), "inventory manifest differs")
        seen_targets = set()
        names = {}
        for artifact in inventory["artifacts"]:
            target = (artifact["package"], artifact["target"], artifact["kind"])
            require(target in targets and target not in seen_targets, f"unknown/duplicate artifact: {target}")
            seen_targets.add(target)
            require(re.fullmatch(r"[0-9a-f]{64}", artifact["sha256"]) and artifact["package_id"] and artifact["executable"], "missing compiled artifact identity")
            if artifact["package"] == "yorishiro":
                require("community" in artifact["features"] and (("enterprise" in artifact["features"]) == (edition == "ee")), "wrong artifact edition")
            tests, ignored = artifact["tests"], artifact["ignored"]
            require(len(tests) == len(set(tests)) and len(ignored) == len(set(ignored)), "duplicate compiled identity")
            require(set(ignored) <= set(tests), "ignored identity missing")
            require(tests or targets[target]["allow_zero"], f"unexpected zero-test artifact: {target}")
            for name in tests:
                key = (artifact["package"], artifact["target"], artifact["kind"], name)
                require(key not in names, f"duplicate compiled key: {key}")
                names[key] = name in ignored
            artifact_records.append({"edition": edition, **artifact})
        require(seen_targets == set(targets), "missing workspace test artifact")
        inventories[edition] = names
    keys = set(inventories["ce"]) | set(inventories["ee"])
    require(set(exact) <= keys, f"stale exact assignments: {set(exact) - keys}")
    # Reviewed ordinary declarations are a second omission check, not a substitute for compiled enumeration.
    for name, source in functions.items():
        if source and source["is_test"]:
            key = ("yorishiro", "mod", "test", name)
            for edition in source["editions"]:
                require(key in inventories[edition], f"missing source-declared test in {edition} inventory: {name}")
    assignments, pending, helpers = [], [], []
    for key in sorted(keys):
        package, target, kind, name = key
        present = [edition for edition in ("ce", "ee") if key in inventories[edition]]
        row = exact.get(key)
        integration = (package, target, kind) == ("yorishiro", "mod", "test")
        source = functions.get(name) if integration else None
        if row and not integration:
            # Nonzero unit/bin/migration artifacts require exact reviewed declaring-file mappings too.
            module = row.get("source_module", "")
            prefix = module + "::" if module else ""
            require(name.startswith(prefix), f"wrong declaring module: {key}")
            source = source_functions(root, row["source"], follow_modules=False).get(name[len(prefix):])
        if row:
            require(present == row["editions"], f"missing/wrong edition for exact identity: {key}")
            require(source is not None and source["is_test"] and source["source"] == row["source"], f"missing/ambiguous exact source: {key}")
        else:
            require(source is not None and source["is_test"] and common.get(source["module"]) == source["source"], f"unknown compiled identity/declaring module: {key}")
            require(present == source["editions"], f"missing/wrong edition common identity: {key}")
        ignored = any(inventories[edition].get(key, False) for edition in present)
        require(not ignored or (row and row["role"] == "resource" and row.get("include_ignored")), f"unclassified ignored identity: {key}")
        owner = "ce" if "ce" in present else "ee"
        role = row["role"] if row else "common"
        require(role in {"common", "postgres", "queue", "licensed", "resource", "pending_split", "helper"}, f"unknown role: {key}")
        entry = {"package": package, "target": target, "kind": kind, "name": name, "compiled_editions": present,
                 "owner": owner, "role": role, "source": source["source"], "ignored": ignored,
                 "jobs": row["jobs"] if row else ["common-sqlite"]}
        if row:
            entry["reason"] = row["reason"]
            if role == "resource":
                entry["include_ignored"] = row.get("include_ignored", False)
            if row.get("provider_parity"):
                entry["provider_parity"] = True
        if role == "pending_split":
            pending.append(entry)
        elif role == "helper":
            parent = row["parent"]
            require((package, target, kind, parent) in inventories[owner], f"missing helper parent: {parent}")
            helpers.append({**entry, "parent": parent, "requires_parent_child_receipt": True})
        else:
            require(entry["jobs"] and len(entry["jobs"]) == len(set(entry["jobs"])), f"missing/duplicate owner jobs: {key}")
            require(all(job in manifest["jobs"] for job in entry["jobs"]), f"unknown responsibility job: {key}")
            require(owner == "ee" or not any(manifest["jobs"][job].get("signed_licence") for job in entry["jobs"]),
                    f"signed-licence contract assigned to CE: {key}")
            if len(entry["jobs"]) > 1:
                require(row and row.get("provider_parity"), f"undeclared duplicate ownership: {key}")
            assignments.append(entry)
    for helper in helpers:
        require(any(entry["package"] == helper["package"] and entry["target"] == helper["target"]
                    and entry["kind"] == helper["kind"] and entry["name"] == helper["parent"]
                    and entry["owner"] == helper["owner"] for entry in assignments),
                f"helper parent has no execution responsibility: {helper['parent']}")
    for name, row in delta.items():
        key = ("yorishiro", "mod", "test", name)
        require(key in inventories["ce"] and key in inventories["ee"], f"stale/missing EE delta: {name}")
        require(all(job in manifest["jobs"] for job in row["jobs"]), f"unknown EE delta job: {name}")
        assignments.append({"package": "yorishiro", "target": "mod", "kind": "test", "name": name, "owner": "ee",
                            "role": "ee_delta", "jobs": row["jobs"], "reason": row["reason"]})
    matrix = []
    for owner, job in sorted({(entry["owner"], job) for entry in assignments for job in entry["jobs"]
                              if not manifest["jobs"][job].get("manual_external")}):
        requirements = manifest["jobs"][job]
        require(isinstance(requirements.get("requires_e5_cache", False), bool), "invalid E5 cache requirement")
        require(not requirements.get("requires_e5_cache") or job == "resource-e5", "unreviewed E5 cache consumer")
        require(job != "resource-e5" or requirements.get("requires_e5_cache"), "E5 contract lacks cache requirement")
        matrix.append({"edition": owner, "job": job, **requirements})
    return {"schema": 1, "inventory_valid": True, "execution_matrix_ready": not pending,
             "manifest_sha256": digest(json.dumps(manifest, sort_keys=True).encode()),
             "cutover_blockers": ["mixed contracts require scenario splits"] if pending else [],
              "execution_matrix": {"include": matrix},
              "core_matrix": {"include": [row for row in matrix if not row.get("requires_e5_cache", False)]},
              "e5_matrix": {"include": [row for row in matrix if row.get("requires_e5_cache", False)]},
            "pending_splits": pending, "assignments": assignments, "helpers": helpers,
             "external_contracts": [entry | {"status": "opt-in-not-required" if any(
                                        manifest["jobs"][job].get("manual_external") for job in entry["jobs"]
                                        ) else "required-execution-receipt"}
                                   for entry in assignments if entry["role"] == "resource"],
            "artifacts": artifact_records, "jobs": manifest["jobs"], "revision": ce["revision"],
            "resource_contracts_are_not_execution_evidence": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    collector = commands.add_parser("collect")
    collector.add_argument("--cargo-messages", type=Path, required=True)
    collector.add_argument("--edition", choices=("ce", "ee"), required=True)
    collector.add_argument("--output", type=Path, required=True)
    verifier = commands.add_parser("verify")
    verifier.add_argument("--ce", type=Path, required=True)
    verifier.add_argument("--ee", type=Path, required=True)
    verifier.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        manifest = load(MANIFEST)
        if args.mode == "collect":
            require(os.environ.get("GITHUB_ACTIONS") == "true", "collect lists test executables only in GitHub Actions")
            result = collect(args.cargo_messages.read_text(), args.edition, manifest)
        else:
            result = verify(load(args.ce), load(args.ee), manifest)
        write(args.output, result)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"test assignments: {error}\n")


if __name__ == "__main__":
    main()
