#!/usr/bin/env python3
"""Run LLVM coverage and emit durable per-source-file coverage artifacts."""

import csv
import json
import os
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_FILTER = r"(^|/)src/models/_entities/|(^|/)migration/src/|(^|/)src/bin/"


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

    command = [
        "cargo",
        toolchain,
        "llvm-cov",
        "--locked",
        "--workspace",
        "--branch",
        "--json",
        "--summary-only",
        "--ignore-filename-regex",
        ignore,
        "--output-path",
        str(raw),
    ]
    subprocess.run(command, cwd=ROOT, check=True)

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
