#!/usr/bin/env python3
"""Run LLVM coverage and emit durable per-source-file coverage artifacts."""

import csv
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_FILTER = r"(^|/)src/models/_entities/|(^|/)migration/src/|(^|/)src/bin/"


def assignment_environment(cell, base, output_dir):
    """Use the same topology and licence axes as ordinary assigned execution."""
    env = dict(base)
    for name in ("YORISHIRO_POSTGRES_TEST_URL", "YORISHIRO_REDIS_TEST_URL",
                 "YORISHIRO_TEST_REDIS_BASE_URL", "YORISHIRO_TEST_E5_MODEL", "YORISHIRO_TEST_E5_TOKENIZER"):
        env.pop(name, None)
    pg = "postgres://yorishiro:yorishiro@localhost:5432/yorishiro"
    name = cell["edition"] + "-" + cell["job"]
    env["DATABASE_URL"] = pg if cell["database"] == "postgres" else f"sqlite://{ROOT}/coverage-{name}.sqlite3?mode=rwc"
    queue = cell["queue"]
    env["QUEUE_URL"] = pg if queue == "postgres" else (
        "redis://localhost:6379/15" if queue == "redis" else f"sqlite://{ROOT}/coverage-{name}-queue.sqlite3?mode=rwc")
    env["LOCO_ENV"] = "test_" + cell["database"] + ("_redis" if queue == "redis" else "")
    env["YORISHIRO_QUEUE_KIND"] = {"postgres": "Postgres", "redis": "Redis", "sqlite": "Sqlite"}[queue]
    env["YORISHIRO_TEST_EDITION"] = cell["edition"]
    env["YORISHIRO_TEST_TOPOLOGY"] = cell["database"] + "-" + queue
    env["YORISHIRO_TEST_QUEUE"] = queue
    env["YORISHIRO_LICENSE_KEY"] = base.get("YORISHIRO_LICENSE_KEY", "") if cell.get("signed_licence") else ""
    if cell.get("signed_licence") and not env["YORISHIRO_LICENSE_KEY"]:
        raise RuntimeError("assigned coverage requires the signed EE licence secret")
    if cell.get("requires_e5_cache"):
        env["YORISHIRO_TEST_E5_MODEL"] = str(ROOT / "models/multilingual-e5-base/model.safetensors")
        env["YORISHIRO_TEST_E5_TOKENIZER"] = str(ROOT / "models/multilingual-e5-base/tokenizer.json")
    env["YORISHIRO_REQUEST_TIMING_DIR"] = str(output_dir / "timings" / (cell["edition"] + "-" + cell["job"]))
    return env


def run_assignments(output_dir, toolchain):
    """Instrument once per edition, then execute the authoritative exact assignments."""
    if os.environ.get("GITHUB_ACTIONS") != "true" or not os.environ.get("LLVM_PROFILE_FILE"):
        raise RuntimeError("coverage requires GHA and cargo llvm-cov show-env instrumentation")
    if pathlib.Path(os.environ.get("CARGO_TARGET_DIR", "")).resolve() != ROOT / "target":
        raise RuntimeError("coverage artifacts must use the reviewed native target directory")

    def run(*command, **kwargs):
        subprocess.run(list(command), cwd=ROOT, check=True, timeout=5400, **kwargs)

    inventories = {}
    for edition, features in (("ce", "community"), ("ee", "enterprise")):
        messages = output_dir / f"cargo-{edition}.jsonl"
        inventory = output_dir / f"inventory-{edition}.json"
        cargo = ("cargo", toolchain)
        with messages.open("w") as stream:
            run(*cargo, "test", "--locked", "--no-default-features", "--features", features,
                "--workspace", "--no-run", "--message-format=json", stdout=stream)
        run(*cargo, "build", "--locked", "--no-default-features", "--features", features, "--bin", "yorishiro")
        run(sys.executable, "scripts/test_assignments.py", "collect", "--cargo-messages", str(messages),
            "--edition", edition, "--output", str(inventory))
        data = json.loads(inventory.read_text())
        # EE's ordinary executable replaces CE's path; preserve both exact producer artifacts.
        for artifact in [*data["artifacts"], data["process_binary"]]:
            path = pathlib.Path(artifact["executable"])
            saved = output_dir / "binaries" / edition / path.relative_to(ROOT)
            saved.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, saved)
        inventories[edition] = inventory

    plan_path = output_dir / "assignments.json"
    run(sys.executable, "scripts/test_assignments.py", "verify", "--ce", str(inventories["ce"]),
        "--ee", str(inventories["ee"]), "--output", str(plan_path))
    plan = json.loads(plan_path.read_text())
    if not plan["execution_matrix_ready"]:
        raise RuntimeError("coverage assignment inventory is not executable")
    for edition in ("ce", "ee"):
        data = json.loads(inventories[edition].read_text())
        for artifact in [*data["artifacts"], data["process_binary"]]:
            path = pathlib.Path(artifact["executable"])
            shutil.copy2(output_dir / "binaries" / edition / path.relative_to(ROOT), path)
        if edition == "ce":
            for suite in ("rest", "mcp", "cli"):
                run(sys.executable, "tests/blackbox/run.py", "--edition", "ce", "--suite", suite,
                    "--binary", str(ROOT / "target/debug/yorishiro"),
                    "--artifacts", str(output_dir / "public" / suite))
            run(sys.executable, "scripts/check_blackbox_reports.py", "--edition", "ce",
                "--binary", str(ROOT / "target/debug/yorishiro"), "--reports", str(output_dir / "public"))
        for cell in plan["execution_matrix"]["include"]:
            if cell["edition"] != edition:
                continue
            # Ordinary GHA cells own fresh services; preserve that isolation in this sequential report.
            if cell["database"] == "postgres":
                run("docker", "exec", os.environ["POSTGRES_CONTAINER"], "psql", "-v", "ON_ERROR_STOP=1",
                    "-U", "postgres", "-d", "postgres", "-c", "DROP DATABASE yorishiro WITH (FORCE)",
                    "-c", "CREATE DATABASE yorishiro OWNER yorishiro")
            if cell["queue"] == "redis":
                run("docker", "exec", "coverage-redis", "valkey-cli", "FLUSHALL")
            directory = output_dir / "receipts" / (edition + "-" + cell["job"])
            run(sys.executable, "scripts/run_assigned_tests.py", "run", "--assignments", str(plan_path),
                "--edition", edition, "--job", cell["job"], "--output", str(directory / "receipt.json"),
                "--artifacts", str(directory / "logs"), env=assignment_environment(cell, os.environ, output_dir))
    run(sys.executable, "scripts/run_assigned_tests.py", "verify", "--assignments", str(plan_path),
        "--receipts", str(output_dir / "receipts"), "--output", str(output_dir / "execution.json"))


def metric(summary, name):
    value = summary.get(name)
    if not isinstance(value, dict) or not {"count", "covered", "percent"} <= value.keys():
        raise RuntimeError(f"LLVM JSON is missing {name} coverage fields")
    return {key: value[key] for key in ("count", "covered", "percent")}


def relative_module(filename):
    path = pathlib.Path(os.path.normpath(filename))
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return os.path.relpath(path, ROOT)


def main():
    output_dir = pathlib.Path(os.environ.get("COVERAGE_OUTPUT_DIR", "coverage"))
    if not output_dir.is_absolute():
        output_dir = ROOT / output_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    raw = output_dir / "llvm-cov.json"
    ignore = os.environ.get("COVERAGE_IGNORE_REGEX", DEFAULT_FILTER)
    toolchain = os.environ.get("COVERAGE_TOOLCHAIN", "+nightly")

    run_assignments(output_dir, toolchain)

    command = [
        "cargo",
        toolchain,
        "llvm-cov",
        "report",
        "--locked",
        "--json",
        "--summary-only",
        "--ignore-filename-regex",
        ignore,
        "--output-path",
        str(raw),
    ]
    subprocess.run(command, cwd=ROOT, check=True, timeout=900)

    report = json.loads(raw.read_text())
    files = report.get("data", [{}])[0].get("files", [])
    modules = []
    for entry in files:
        summary = entry.get("summary", {})
        modules.append(
            {
                "module": relative_module(entry["filename"]),
                "lines": metric(summary, "lines"),
                "functions": metric(summary, "functions"),
                "branches": metric(summary, "branches"),
            }
        )
    modules.sort(key=lambda item: item["module"])

    totals = {}
    for name in ("lines", "functions", "branches"):
        count = sum(item[name]["count"] for item in modules)
        covered = sum(item[name]["covered"] for item in modules)
        totals[name] = {
            "count": count,
            "covered": covered,
            "percent": (covered * 100 / count) if count else 100.0,
        }

    artifact = {
        "schema": "yorishiro.actionable-coverage.v1",
        "ignore_filename_regex": ignore,
        "toolchain": toolchain,
        "totals": totals,
        "modules": modules,
    }
    (output_dir / "modules.json").write_text(json.dumps(artifact, indent=2) + "\n")

    with (output_dir / "modules.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(
            [
                "module",
                "lines_percent",
                "lines_covered",
                "lines_count",
                "functions_percent",
                "functions_covered",
                "functions_count",
                "branches_percent",
                "branches_covered",
                "branches_count",
            ]
        )
        for item in modules:
            writer.writerow(
                [
                    item["module"],
                    item["lines"]["percent"],
                    item["lines"]["covered"],
                    item["lines"]["count"],
                    item["functions"]["percent"],
                    item["functions"]["covered"],
                    item["functions"]["count"],
                    item["branches"]["percent"],
                    item["branches"]["covered"],
                    item["branches"]["count"],
                ]
            )

    lowest = sorted(
        modules,
        key=lambda item: (
            item["lines"]["percent"],
            item["functions"]["percent"],
            item["branches"]["percent"],
            item["module"],
        ),
    )[:20]
    with (output_dir / "lowest-covered.txt").open("w") as stream:
        stream.write("Lowest-covered actionable source modules\n")
        stream.write("module | lines | functions | branches\n")
        for item in lowest:
            stream.write(
                f"{item['module']} | {item['lines']['percent']:.2f}% "
                f"({item['lines']['covered']}/{item['lines']['count']}) | "
                f"{item['functions']['percent']:.2f}% "
                f"({item['functions']['covered']}/{item['functions']['count']}) | "
                f"{item['branches']['percent']:.2f}% "
                f"({item['branches']['covered']}/{item['branches']['count']})\n"
            )

    print(json.dumps(totals, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (KeyError, RuntimeError, json.JSONDecodeError) as error:
        print(f"coverage report failed: {error}", file=sys.stderr)
        sys.exit(1)
