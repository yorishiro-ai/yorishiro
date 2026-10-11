#!/usr/bin/env -S uv run
"""Deliberate violations prove the CI matrix gate rejects regressions."""

from check_ci_matrix import ROOT, validate


def main() -> int:
    workflow = (ROOT / ".github/workflows/ci.yml").read_text()
    validate(workflow)
    matrix_start = workflow.index("          - edition: ce", workflow.index("\n  test:\n"))
    first_end = workflow.index("          - edition: ce", matrix_start + 1)
    first = workflow[matrix_start:first_end]
    violations = [
        workflow[:matrix_start] + workflow[first_end:],  # Missing pair.
        workflow[:matrix_start] + first + workflow[matrix_start:],  # Duplicate pair.
        workflow.replace("queue_key: postgres", "queue_key: sqlite", 1),  # Crossed SQL pair.
        workflow.replace("        include:", "        edition: [ce, ee]\n        include:", 1),  # Original axis/include bug.
        workflow.replace("${{ matrix.edition }}", "hidden-edition"),
    ]
    for invalid in violations:
        try:
            validate(invalid)
        except (AssertionError, IndexError):
            continue
        raise AssertionError("CI matrix gate accepted a deliberate violation")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
