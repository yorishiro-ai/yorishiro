# Test assignment checks

Public contracts run the real binary through `tests/blackbox/run.py`.
Their reports are checked by `scripts/check_blackbox_reports.py` against the exact scenario list and executable hash.
Common public contracts run once on CE SQLite; enterprise composition and backend guarantees need their own explicit assignments, not another full cross-product.

The compiled inventory is a separate check.
GHA builds CE and EE workspace test targets with Cargo's JSON message format, locates the reported executables, and enumerates their actual libtest identities.
`scripts/test_assignments.py` joins both inventories to `assignments.json`.
New, missing, stale or ambiguous assignments must fail rather than silently select zero tests.
An inventory receipt proves classification, not execution of the classified test.

Assignment-driven execution passed GHA at `5730011b` before retiring the legacy eight full-suite cells.
Mixed common/backend tests now have separate identities, including distinct-tenant assertions in the PostgreSQL cases.
CE and EE test executables are built once per edition and archived with the main binary for native Ubuntu 26.04 consumers.
Public contracts reuse the CE main binary instead of rebuilding it.
`assigned-tests.yml` provisions only the selected PostgreSQL or Redis service and runs the exact selected identities.
`run_assigned_tests.py` checks binary hashes and listing completeness before execution, then requires successful execution receipts for every assigned identity/topology.
Assigned backend/provider mismatches fail instead of returning a green test with no assertions.
Known resource-dependent tests and subprocess helpers must remain visible in the inventory rather than disappear through a broad ignore rule.
Pinned E5 model and reference contracts are mandatory; the pre-existing private live-endpoint smoke remains explicitly opt-in and is never counted as passed by ordinary CI.

## Responsibility ownership

Common assertions have one CE/SQLite owner when compiled in both editions.
EE-only assertions use SQLite unless PostgreSQL semantics or multi-tenant fixtures require PostgreSQL.
An explicit `ee_delta` records the shared identities whose enterprise branches also need execution.
Provider parity is allowed only for the exact identities marked `provider_parity`, not a whole worker or request module.
Shared EE PostgreSQL fixtures create independent production tenants, so their owner needs licensed multi-tenancy even when an individual operation has no HTTP licence gate.
Unlicensed and signed-licence SQLite contracts remain separate.

Add tests through their declaring-module assignment or an exact reviewed exception in `assignments.json`.
Unknown compiled identities, stale exceptions and unreviewed Rust-source changes fail the assignment check.
The source fingerprint is a conservative review gate, not a proof of arbitrary Rust control flow.
After reviewing changed backend, provider and feature branches, refresh the fingerprint rather than disabling the guard.

The GHA build produces `test-inventory-ce`, `test-inventory-ee` and matching `test-executables-*` artifacts.
The assignment join produces `test-assignments/test-assignments.json` with the exact responsibility matrix.
Each consumer writes `assigned-test-result-<edition>-<job>` with its execution receipt and redacted logs.
The required aggregate compares all receipts with that complete plan; classification alone cannot satisfy it.

To reproduce a selected responsibility inside its prepared GHA topology:

```sh
python3 scripts/run_assigned_tests.py run --assignments /absolute/test-assignments.json --edition ce --job common-sqlite --output /absolute/receipt.json --artifacts /absolute/logs
```

Use the matching archived executable set and commit, not a stale binary from another local build.
The runner intentionally requires the GHA environment during this migration.
Public REST/MCP/CLI commands and fixture boundaries are documented in `tests/blackbox/README.md`.

## Runtime policy

Application suites and runner unit tests execute only in GHA during this migration.
Local work is limited to the repository's static checks.
Do not add runtime unittest execution to `make check-all` or `make check-all-ee`.

## Internal harness dependency

The external runner does not make all in-process fixtures obsolete.
Configuration/provider stubs in `tests/config/load.rs` and `tests/config/local_embedding_resolution.rs`, PostgreSQL creation helpers in `tests/db/migrations.rs`, and retained request/worker race and fault fixtures still use Loco's testing APIs.
Keep the `testing` dev feature while those callers remain.
The lock currently selects `axum-test 17.3.0`; adding an incompatible second TestServer or a dependency-only framework patch is not part of this migration.

## Evidence

Keep per-group logs, stable identities, exact commit and binary hashes, and timing artifacts.
Compare total runner cost and wall time only after the replacement covers the legacy guarantees.
Fast small public scenarios alone are not evidence that the whole suite became faster.
