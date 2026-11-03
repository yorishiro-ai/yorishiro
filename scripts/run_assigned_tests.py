#!/usr/bin/env python3
"""Execute native compiled assignments, or verify the complete set of execution receipts."""

import argparse
import contextlib
import datetime
import hashlib
import importlib.util
import json
import os
import re
import signal
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("assigned_inventory", ROOT / "scripts/test_assignments.py")
inventory = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(inventory)
require = inventory.require
TARGET = ("package", "target", "kind")
IDENTITY = (*TARGET, "name")
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
RESULT = re.compile(r"\btest ([^\s]+)(?: - should panic)? \.\.\. ")
DATABASE_DDL = re.compile(r"\b(CREATE|DROP)\s+DATABASE\s", re.IGNORECASE)
DDL_PROBE = "CREATE TEMP TABLE assigned_shared_ddl_probe (id integer)"


def fingerprint(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def key(row, fields=IDENTITY):
    return tuple(row[field] for field in fields)


def utc_now():
    return datetime.datetime.now(datetime.UTC).isoformat()


def optional_job(plan, job, manifest):
    metadata = plan.get("optional_jobs", manifest.get("optional_jobs", {}))
    if isinstance(metadata, list):
        metadata = {row["job"]: row for row in metadata}
    return (plan["jobs"][job].get("manual_external", False) or
            plan["jobs"][job].get("status") == "opt-in-not-required" or
            metadata.get(job, {}).get("status") == "opt-in-not-required")


def manual_exclusions(plan, manifest):
    return [{**{field: row[field] for field in IDENTITY}, "owner": row["owner"], "job": job,
             "status": "opt-in-not-required", "counted_passed": False}
            for row in plan["assignments"] for job in row["jobs"]
             if row["role"] == "manual_external" or optional_job(plan, job, manifest)]


def postgres_dedicated(plan, manifest):
    """All other PG assignments are reviewed shared fixtures, frozen by source_guard."""
    entries = manifest.get("postgres_dedicated", [])
    names = [row["name"] for row in entries]
    require(len(names) == len(set(names)), "duplicate PostgreSQL dedicated identity")
    pg_names = {row["name"] for row in plan["assignments"]
                if any(plan["jobs"][job]["database"] == "postgres" for job in row["jobs"])}
    require(set(names) <= pg_names, "unknown PostgreSQL dedicated identity")
    declarations = {(name, row["source"]) for row in manifest.get("exception_groups", []) for name in row["names"]}
    declarations.update((row["name"], row["source"]) for row in manifest.get("exceptions", []))
    require(all((row["name"], row["source"]) in declarations for row in entries),
            "PostgreSQL dedicated identity lacks source classification")
    require(not pg_names or "postgres_dedicated" in manifest, "missing PostgreSQL fixture classification")
    return set(names)


def fixture_group(row, config, dedicated):
    return ("dedicated" if row["name"] in dedicated else "shared") if config["database"] == "postgres" else "default"


def provenance(plan, manifest):
    require(plan["schema"] == 1 and plan["inventory_valid"] is True, "invalid assignment inventory")
    require(plan["jobs"] == manifest["jobs"], "assignment topology differs from manifest")
    if "optional_jobs" in plan:
        require(plan["optional_jobs"] == manifest.get("optional_jobs", {}), "optional jobs differ from manifest")
    manifest_hash = fingerprint(manifest)
    require(plan.get("manifest_sha256", manifest_hash) == manifest_hash, "assignment manifest hash differs")
    artifacts = {}
    for row in plan["artifacts"]:
        identity = (row["edition"], *key(row, TARGET))
        require(identity not in artifacts, "duplicate inventory artifact")
        require(row["edition"] in {"ce", "ee"}, "wrong inventory edition")
        require(re.fullmatch(r"[0-9a-f]{64}", row["sha256"]), "invalid binary hash")
        require(len(row["tests"]) == len(set(row["tests"])), "duplicate inventory test")
        require(set(row["ignored"]) <= set(row["tests"]), "unknown ignored identity")
        if row["package"] == "yorishiro":
            require("community" in row["features"] and
                    (("enterprise" in row["features"]) == (row["edition"] == "ee")), "wrong binary edition")
        artifacts[identity] = row
    expected = {}
    for row in plan["assignments"]:
        require(row["owner"] in {"ce", "ee"} and row["jobs"], "invalid assignment owner/jobs")
        require(row["role"] not in {"helper", "pending_split"}, "non-executable assignment")
        require(len(row["jobs"]) == len(set(row["jobs"])), "duplicate owner job")
        binary = artifacts[(row["owner"], *key(row, TARGET))]
        require(row["name"] in binary["tests"], "assigned identity absent from binary")
        for job in row["jobs"]:
            require(job in plan["jobs"], "unknown responsibility job")
            if row["role"] == "manual_external" or optional_job(plan, job, manifest):
                continue
            identity = (row["owner"], job, *key(row))
            require(identity not in expected, "duplicate assignment")
            expected[identity] = row
    require(expected, "zero required assignment identities")
    postgres_dedicated(plan, manifest)
    if "execution_matrix" in plan:
        cells = plan["execution_matrix"]["include"]
        expected_cells = [{"edition": edition, "job": job, **plan["jobs"][job]}
                          for edition, job in sorted({identity[:2] for identity in expected})]
        require(sorted(cells, key=lambda cell: (cell["edition"], cell["job"])) == expected_cells,
                "execution matrix differs from required owner/job assignments")
    for helper in plan.get("helpers", []):
        require(not any(key(row) == key(helper) for row in plan["assignments"]), "helper assigned independently")
        require(any(row["owner"] == helper["owner"] and key(row, TARGET) == key(helper, TARGET)
                    and row["name"] == helper["parent"] for row in plan["assignments"]), "helper parent not assigned")
    return {"revision": plan["revision"], "assignments_sha256": fingerprint(plan),
            "manifest_sha256": manifest_hash,
            "inventory_sha256": fingerprint(plan["artifacts"])}, artifacts, expected


def topology_environment(job, edition, env):
    """Validate explicit inputs before starting even a listing process."""
    env = dict(env)
    database, queue = job["database"], job["queue"]
    require((database, queue) in {("sqlite", "sqlite"), ("postgres", "postgres"),
                                ("sqlite", "redis"), ("postgres", "redis")}, "unsupported topology")
    def backend(url):
        if url.startswith("sqlite:"):
            return "sqlite"
        if url.startswith(("postgres://", "postgresql://")):
            return "postgres"
        if url.startswith(("redis://", "rediss://")):
            return "redis"
        return None

    require(backend(env.get("DATABASE_URL", "")) == database, "DATABASE_URL topology mismatch")
    require(backend(env.get("QUEUE_URL", "")) == queue, "QUEUE_URL topology mismatch")
    if queue == "sqlite":
        def sqlite_path(url):
            return Path(url.removeprefix("sqlite://").removeprefix("sqlite:").split("?", 1)[0]).resolve()
        require(sqlite_path(env["DATABASE_URL"]) != sqlite_path(env["QUEUE_URL"]),
                "SQLite application and queue must use separate files")
    expected_env = "test_" + database + ("_redis" if queue == "redis" else "")
    require(env.get("LOCO_ENV") == expected_env, "LOCO_ENV topology mismatch")
    require(env.get("YORISHIRO_TEST_EDITION", edition) == edition, "environment edition mismatch")
    require(env.get("YORISHIRO_TEST_TOPOLOGY", f"{database}-{queue}") == f"{database}-{queue}",
            "environment topology mismatch")
    require(env.get("YORISHIRO_TEST_QUEUE", queue) == queue, "environment queue mismatch")
    queue_kind = {"sqlite": "Sqlite", "postgres": "Postgres", "redis": "Redis"}[queue]
    require(env.get("YORISHIRO_QUEUE_KIND", queue_kind) == queue_kind, "environment queue kind mismatch")
    licensed = bool(job.get("signed_licence", False))
    require(bool(env.get("YORISHIRO_LICENSE_KEY")) == licensed, "signed licence topology mismatch")
    require(not licensed or edition == "ee", "signed licence requires EE")
    env.update(YORISHIRO_TEST_EDITION=edition, YORISHIRO_TEST_TOPOLOGY=f"{database}-{queue}",
               YORISHIRO_TEST_QUEUE=queue, YORISHIRO_TEST_STRICT_TOPOLOGY="true")
    if database == "postgres":
        require(env.get("YORISHIRO_POSTGRES_TEST_URL", env["DATABASE_URL"]) == env["DATABASE_URL"],
                "PostgreSQL helper URL differs from application URL")
        env["YORISHIRO_POSTGRES_TEST_URL"] = env["DATABASE_URL"]
    else:
        require(not env.get("YORISHIRO_POSTGRES_TEST_URL"), "unexpected PostgreSQL helper URL")
    if queue == "redis":
        for name in ("YORISHIRO_REDIS_TEST_URL", "YORISHIRO_TEST_REDIS_BASE_URL"):
            require(env.get(name, env["QUEUE_URL"]) == env["QUEUE_URL"], "Redis helper URL differs from queue URL")
            env[name] = env["QUEUE_URL"]
    else:
        require(not env.get("YORISHIRO_REDIS_TEST_URL") and not env.get("YORISHIRO_TEST_REDIS_BASE_URL"),
                "unexpected Redis helper URL")
    return env, {"database": database, "queue": queue, "signed_licence": licensed,
                 "loco_env": expected_env, "strict": True}


def relocated_binary(artifact, root):
    original = Path(artifact["executable"])
    require(original.parent.parts[-3:] == ("target", "debug", "deps"), "non-native inventory executable path")
    root = Path(root).resolve()
    path = (root / "target/debug/deps" / original.name).resolve()
    require(path.is_relative_to(root), "artifact escapes download directory")
    require(path.is_file() and os.access(path, os.X_OK), "missing/non-executable downloaded binary")
    require(inventory.file_digest(path) == artifact["sha256"], "downloaded binary hash differs")
    return path


def selection_flags(artifact, selected):
    names = {row["name"] for row in selected}
    require(names and names <= set(artifact["tests"]), "empty/unknown selected identities")
    ignored = names & set(artifact["ignored"])
    require(all(row["role"] == "resource" and row.get("include_ignored")
                for row in selected if row["name"] in ignored), "ignored identity lacks resource permission")
    # --exact also makes each --skip an equality check, not a substring filter.
    flags = ["--exact"]
    for name in sorted(set(artifact["tests"]) - names):
        flags.extend(["--skip", name])
    if ignored:
        require(all(row["role"] == "resource" for row in selected), "include-ignored outside resource job")
        flags.append("--include-ignored")
    return flags


def redact(text, env):
    values = {value for name, value in env.items() if value and
              any(token in name for token in ("KEY", "TOKEN", "SECRET", "PASSWORD", "URL"))}
    for value in sorted(values, key=len, reverse=True):
        text = text.replace(value, "[REDACTED]")
    text = re.sub(r"(?i)\b(postgres(?:ql)?|rediss?|https?)://[^\s\"'<>]+", r"\1://[REDACTED]", text)
    return re.sub(r"(?i)(authorization\s*[:=]\s*bearer\s+)\S+", r"\1[REDACTED]", text)


def process(argv, env, deadline, log):
    """Keep raw output in an anonymous private file, and persist only redacted logs."""
    remaining = deadline - time.monotonic()
    require(remaining > 0, "job deadline exceeded")
    timed_out = False
    with tempfile.TemporaryFile(mode="w+b") as raw:
        child = subprocess.Popen(argv, cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
                                 stdout=raw, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            try:
                child.wait(timeout=remaining)
            except subprocess.TimeoutExpired:
                timed_out = True
        finally:
            # Kill the whole group even after normal exit to remove surviving subprocess helpers.
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            raw.seek(0)
            text = raw.read().decode("utf-8", errors="replace")
            Path(log).write_text(redact(text, env))
    require(not timed_out, "job deadline exceeded; process group terminated")
    return child.returncode, text


def passed_tests(text, names):
    text = ANSI.sub("", text)
    starts = list(RESULT.finditer(text))
    results = []
    for index, match in enumerate(starts):
        stop = starts[index + 1].start() if index + 1 < len(starts) else len(text)
        # --nocapture output can interrupt the formatter between the name and its result.
        statuses = re.findall(r"(?:^|[ \t])(ok|FAILED|ignored)(?:, [^\n]*)?[ \t]*(?=\r?$)",
                              text[match.end():stop], re.MULTILINE)
        require(len(statuses) == 1, "missing/ambiguous per-test status")
        results.append((match[1], statuses[0]))
    require(len(results) == len(names) and {name for name, _ in results} == set(names),
            "missing/duplicate/unexpected per-test results")
    require(all(status == "ok" for _, status in results), "failed or ignored test result")
    summaries = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", text)
    require(summaries == [(str(len(names)), "0", "0")], "missing/incorrect libtest success summary")
    return sorted(name for name, _ in results)


def helper_evidence(plan, edition, rows):
    parents = {key(row) for row in rows}
    return [{"name": helper["name"], "parent": helper["parent"], **{field: helper[field] for field in TARGET},
             "evidence": "reviewed-parent-assertions", "parent_status": "passed"}
            for helper in plan.get("helpers", []) if helper["owner"] == edition and
             (*key(helper, TARGET), helper["parent"]) in parents]


@contextlib.contextmanager
def shared_ddl_window(env, deadline, logs, index, run_process, evidence):
    """Observe the entire shared process, including its first OnceCell migration."""
    container = env.get("POSTGRES_CONTAINER")
    require(container, "missing POSTGRES_CONTAINER for shared DDL observation")
    since = utc_now()
    code, _ = run_process(["docker", "exec", container, "psql", "-v", "ON_ERROR_STOP=1",
                           "-U", "yorishiro", "-d", "yorishiro", "-c", DDL_PROBE],
                          env, deadline, logs / f"{index}-ddl-probe.log")
    evidence.update(probe_exit_code=code, since=since)
    try:
        require(code == 0, "PostgreSQL DDL positive probe failed")
        yield
    finally:
        log = logs / f"{index}-shared-ddl.log"
        code, text = run_process(["docker", "logs", "--since", since, container], env, deadline, log)
        evidence.update(log=str(log), logs_exit_code=code, status="failed")
        require(code == 0, "could not collect PostgreSQL DDL logs")
        require(DDL_PROBE in text, "PostgreSQL DDL logging was not observed")
        require(not DATABASE_DDL.search(text), "shared PostgreSQL group created or dropped a database")
        evidence["status"] = "passed" if evidence["probe_exit_code"] == 0 else "failed"


def execute_job(plan, edition, job, artifacts_dir, output, manifest, env, revision,
                timeout_seconds=1500, run_process=process, binary_root=ROOT):
    provenance_fields, artifacts, expected = provenance(plan, manifest)
    require(revision == plan["revision"], "checkout revision differs from inventory")
    require(edition in {"ce", "ee"} and job in plan["jobs"], "unknown edition/job")
    env, topology = topology_environment(plan["jobs"][job], edition, env)
    selected = [row for identity, row in expected.items() if identity[:2] == (edition, job)]
    require(selected, "job selected zero identities")
    require(timeout_seconds > 0, "invalid deadline")
    if job == "resource-e5":
        for name in ("YORISHIRO_TEST_E5_MODEL", "YORISHIRO_TEST_E5_TOKENIZER"):
            require(env.get(name) and Path(env[name]).is_file(), f"missing resource: {name}")
    if job == "resource-live":
        require(env.get("YORISHIRO_EMBEDDING_PROVIDER") == "openai" and
                env.get("YORISHIRO_EMBEDDING_API_KEY") and env.get("YORISHIRO_EMBEDDING_BASE_URL"),
                "live provider resource is not configured")
    dedicated = postgres_dedicated(plan, manifest)
    groups = {}
    for row in selected:
        fixture = fixture_group(row, plan["jobs"][job], dedicated)
        groups.setdefault((*key(row, TARGET), fixture), []).append(row)
    # Include zero-test artifacts in download verification, but never execute them as coverage.
    binaries = {identity[1:]: relocated_binary(artifact, binary_root)
                for identity, artifact in artifacts.items() if identity[0] == edition}
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    logs = Path(artifacts_dir)
    logs.mkdir(parents=True, exist_ok=False)
    deadline = time.monotonic() + timeout_seconds
    started = utc_now()
    receipt = {"schema": 1, **provenance_fields, "edition": edition, "job": job,
               "topology": topology, "started_at": started, "status": "failed", "tests": [],
               "helpers": [], "binaries": []}
    try:
        for index, (group, rows) in enumerate(sorted(groups.items())):
            target, fixture = group[:3], group[3]
            binary = artifacts[(edition, *target)]
            flags = selection_flags(binary, rows)
            prefix = [str(binaries[target]), *flags]
            listing_log, execution_log = logs / f"{index}-list.log", logs / f"{index}-tests.log"
            code, listing = run_process([*prefix, "--list", "--format", "terse"], env, deadline, listing_log)
            names = sorted(row["name"] for row in rows)
            before = utc_now()
            binary_receipt = {**dict(zip(TARGET, target)), "sha256": binary["sha256"], "fixture": fixture,
                              "tests": names, "listing_log": str(listing_log), "listing_exit_code": code,
                              "log": str(execution_log), "started_at": before}
            receipt["binaries"].append(binary_receipt)
            require(code == 0 and inventory.parse_listing(listing) == names, "filtered listing differs from exact assignment")
            observation = binary_receipt.setdefault("ddl_observation", {}) if fixture == "shared" else None
            window = (shared_ddl_window(env, deadline, logs, index, run_process, observation)
                      if fixture == "shared" else contextlib.nullcontext())
            with window:
                code, text = run_process([*prefix, "--nocapture", "--test-threads", "4", "--color", "never"],
                                         env, deadline - (15 if fixture == "shared" else 0), execution_log)
                binary_receipt["exit_code"] = code
            binary_receipt["finished_at"] = utc_now()
            require(code == 0, f"libtest process failed; see {execution_log}")
            passed_tests(text, names)
            after = utc_now()
            for row in rows:
                receipt["tests"].append({**{field: row[field] for field in IDENTITY}, "owner": edition,
                                          "job": job, "status": "passed", "sha256": binary["sha256"],
                                          "started_at": before, "finished_at": after, "topology": topology})
        receipt["helpers"] = helper_evidence(plan, edition, selected)
        receipt["status"] = "passed"
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        receipt["error"] = redact(str(error), env)
        raise
    finally:
        receipt["finished_at"] = utc_now()
        inventory.write(output, receipt)
    return receipt


def aggregate(plan, receipts, manifest):
    fields, artifacts, expected = provenance(plan, manifest)
    dedicated = postgres_dedicated(plan, manifest)
    seen, jobs = set(), set()
    for receipt in receipts:
        require(receipt["schema"] == 1 and receipt["status"] == "passed", "failed execution receipt")
        require(all(receipt.get(name) == value for name, value in fields.items()), "receipt provenance differs")
        edition, job = receipt["edition"], receipt["job"]
        require((edition, job) not in jobs, "duplicate job receipt")
        jobs.add((edition, job))
        require(job in plan["jobs"], "unknown receipt job")
        config = plan["jobs"][job]
        topology = {"database": config["database"], "queue": config["queue"],
                    "signed_licence": bool(config.get("signed_licence", False)), "strict": True,
                    "loco_env": "test_" + config["database"] + ("_redis" if config["queue"] == "redis" else "")}
        require(receipt["topology"] == topology, "receipt topology differs")
        rows = []
        require(receipt["tests"], "zero-test receipt")
        for row in receipt["tests"]:
            identity = (edition, job, *key(row))
            require(identity in expected, "wrong edition/job or unexpected test receipt")
            require(identity not in seen, "duplicate test receipt")
            require(row["status"] == "passed" and row["owner"] == edition and row["job"] == job,
                    "failed/skipped/wrong-owner receipt")
            require(row["topology"] == topology, "test topology differs")
            require(row["sha256"] == artifacts[(edition, *key(row, TARGET))]["sha256"], "receipt binary hash differs")
            require(row["started_at"] <= row["finished_at"] and row["started_at"] >= receipt["started_at"]
                    and row["finished_at"] <= receipt["finished_at"], "invalid receipt timing")
            seen.add(identity)
            rows.append(expected[identity])
        require(receipt["helpers"] == helper_evidence(plan, edition, rows), "missing/wrong helper parent evidence")
        binaries = {}
        for binary in receipt["binaries"]:
            target = key(binary, TARGET)
            group = (*target, binary["fixture"])
            require(group not in binaries, "duplicate binary fixture receipt")
            binaries[group] = binary
            require(binary["sha256"] == artifacts[(edition, *target)]["sha256"], "binary receipt hash differs")
            require(binary["listing_exit_code"] == 0 and binary["exit_code"] == 0, "failed binary receipt")
            require(binary["tests"] == sorted(row["name"] for row in rows if key(row, TARGET) == target
                                             and fixture_group(row, config, dedicated) == binary["fixture"]),
                    "binary receipt identities differ")
            if binary["fixture"] == "shared":
                observation = binary.get("ddl_observation", {})
                require(observation.get("status") == "passed" and observation.get("probe_exit_code") == 0
                        and observation.get("logs_exit_code") == 0 and observation.get("log") and observation.get("since"),
                        "missing/failed PostgreSQL DDL observation")
        require(set(binaries) == {(*key(row, TARGET), fixture_group(row, config, dedicated)) for row in rows},
                "missing binary fixture receipt")
    require(seen == set(expected), f"missing execution receipts: {len(set(expected) - seen)}")
    require(not plan.get("pending_splits"), "pending scenario splits block execution success")
    return {"schema": 1, **fields, "status": "passed", "execution_matrix_ready": True,
            "tests_passed": len(seen), "jobs_passed": len(jobs), "pending_splits": [],
            "excluded_manual_external": manual_exclusions(plan, manifest), "finished_at": utc_now()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("--edition", choices=("ce", "ee"), required=True)
    runner.add_argument("--job", required=True)
    runner.add_argument("--artifacts", type=Path, required=True, help="directory for redacted execution logs")
    runner.add_argument("--timeout-seconds", type=int, default=1500)
    runner.add_argument("--job-timeout-seconds", type=int, default=1800)
    merger = commands.add_parser("verify", aliases=["aggregate"])
    merger.add_argument("--receipts", type=Path, nargs="+", required=True)
    for command in (runner, merger):
        command.add_argument("--assignments", type=Path, required=True)
        command.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        plan = inventory.load(args.assignments)
        manifest = inventory.load(inventory.MANIFEST)
        if args.mode == "run":
            require(os.environ.get("GITHUB_ACTIONS") == "true", "native assigned tests execute only in GitHub Actions")
            require(0 < args.timeout_seconds <= args.job_timeout_seconds - 60, "deadline must leave job cleanup time")
            require(inventory.source_census(ROOT) == manifest["source_guard"], "source guard differs")
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            require(os.environ.get("GITHUB_SHA", revision) == revision, "GitHub revision differs from checkout")
            execute_job(plan, args.edition, args.job, args.artifacts, args.output, manifest,
                        os.environ, revision, args.timeout_seconds)
        else:
            paths = []
            for path in args.receipts:
                found = sorted(path.rglob("receipt.json")) if path.is_dir() else [path]
                require(found, f"no receipt.json files in {path}")
                paths.extend(found)
            result = aggregate(plan, [inventory.load(path) for path in paths], manifest)
            args.output.parent.mkdir(parents=True, exist_ok=True)
            inventory.write(args.output, result)
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"assigned tests: {redact(str(error), os.environ)}\n")


if __name__ == "__main__":
    main()
