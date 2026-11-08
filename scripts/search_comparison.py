"""Run the issue 599 matched diagnostic in GHA, outside assignment receipts."""

import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path


def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("Search comparison runs only in GHA")
    root = Path.cwd()
    output = Path(os.environ["RUNNER_TEMP"]) / "search-comparison"
    output.mkdir()
    baseline = "ebea0ee12f42edf390c0fa897e35cf32fd9dec9d"
    candidate = "eee6dc8cfc523b30ec56c3f3b5276f96e553c992"
    fixture = root / "tests/requests/mcp.rs"
    evidence = {"baseline": baseline, "candidate": candidate, "profile": "release",
                "scope": "Fixed-count mixed REST/MCP workload, stub inference, in-process HTTP; not an open-loop SLO or live inference benchmark",
                "fixture_sha256": hashlib.sha256(fixture.read_bytes()).hexdigest(), "runs": []}
    evidence["env_registry_sha256"] = hashlib.sha256((root / "tests/env.rs").read_bytes()).hexdigest()
    executables = {}
    for label, revision in (("A", baseline), ("B", candidate)):
        checkout = output / label
        run(["git", "worktree", "add", "--detach", str(checkout), revision])
        shutil.copyfile(fixture, checkout / "tests/requests/mcp.rs")
        shutil.copyfile(root / "tests/env.rs", checkout / "tests/env.rs")
        run(["python3", "patches/loco-rs/verify.py", "--prepare"], cwd=checkout)
        build_log = output / f"{label}-build.jsonl"
        with build_log.open("w") as log:
            run(["cargo", "test", "--locked", "--release", "--no-default-features",
                 "--features", "enterprise", "--test", "mod", "--no-run", "--message-format=json"],
                cwd=checkout, stdout=log)
        artifacts = [json.loads(line) for line in build_log.read_text().splitlines()]
        executable = next(Path(row["executable"]) for row in artifacts
                          if row.get("reason") == "compiler-artifact" and row.get("executable")
                          and row.get("target", {}).get("name") == "mod")
        saved = output / f"{label}-test"
        shutil.copyfile(executable, saved)
        saved.chmod(0o755)
        executables[label] = (saved, checkout)
        evidence[f"{label}_binary_sha256"] = hashlib.sha256(saved.read_bytes()).hexdigest()
    with (output / "compiler.txt").open("w") as compiler:
        run(["rustc", "--version", "--verbose"], stdout=compiler)
    try:
        for concurrency in (4, 8):
            for position, label in enumerate(("A", "B", "B", "A")):
                executable, checkout = executables[label]
                log_path = output / f"c{concurrency}-{position}-{label}.log"
                env = dict(os.environ, YORISHIRO_SEARCH_COMPARISON=str(concurrency))
                with log_path.open("w") as log:
                    run([str(executable), "requests::mcp::postgres_search_returns_the_nearest_document_across_code_collapse_and_recovery",
                         "--exact", "--nocapture", "--test-threads=1"], cwd=checkout, env=env,
                        stdout=log, stderr=subprocess.STDOUT, timeout=600)
                results = [json.loads(line.removeprefix("SEARCH_COMPARISON "))
                           for line in log_path.read_text().splitlines() if line.startswith("SEARCH_COMPARISON ")]
                if len(results) != 1 or results[0]["requests"] != concurrency * 32:
                    raise RuntimeError("Missing complete diagnostic measurement")
                evidence["runs"].append(dict(results[0], implementation=label, position=position))
    finally:
        (output / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
