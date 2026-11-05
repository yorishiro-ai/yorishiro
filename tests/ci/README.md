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
Independent CE and EE producers share `test-build.yml`, but CE public contracts and schema regeneration do not wait for the EE build.
The CE producer publishes the small public binary archive directly, without a download/repack job.
Migration definitions have no edition features, so one PostgreSQL regeneration checks their shared schema while SQLite migration contracts remain assigned tests.
CE and EE strict Clippy both compile on the declared minimum compiler; validation rejects disagreement between that compiler and `rust-toolchain.toml` instead of repeating identical MSRV builds.
`assigned-tests.yml` provisions only the selected PostgreSQL or Redis service and runs the exact selected identities.
`run_assigned_tests.py` checks binary hashes and listing completeness before execution, then requires successful execution receipts for every assigned identity/topology.
Both inventories also record the ordinary CLI executable hash.
Consumers verify that main binary before launching any test, including tests that spawn it through `CARGO_BIN_EXE_yorishiro`.
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

Only `resource-e5` declares `requires_e5_cache`.
The assignment join partitions the complete matrix into disjoint ordinary and E5 consumers without changing identity ownership.
Ordinary consumers do not wait for model preparation or restore model files.
E5 consumers wait for the pinned-cache producer, restore the exact cache, and verify it before and after execution.
The complete receipt aggregate still requires both populations and rejects missing or duplicate identities.
Reference-generator Python imports have a separate path-selected and weekly workflow; building that image does not produce the Rust tests' pinned model cache.

Internal consumers let their fixtures own migrations rather than migrating an unrelated outer database first.
Shared PostgreSQL fixtures migrate their existing database through their process-local bootstrap; isolated deployments migrate their own databases.
Raw scheduler-lock consumers use advisory locks without schema tables, and E5 consumers need no database schema.
PostgreSQL role/extension provisioning and the shared-fixture database-DDL observation remain mandatory.
Public CLI bootstrap and schema regeneration still exercise the actual migration command.

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
## Assigned weekly coverage

The informational coverage workflow instruments CE and EE with the same nightly compiler, builds each edition once, and runs the verified assignment plan rather than a PostgreSQL copy of every test.
It preserves each edition's ordinary CLI binary before the other edition replaces its path.
The existing assignment runner enforces backend, queue, licence, executable hashes, shared-fixture DDL observations and complete execution receipts.
Only signed-licence assignments receive the EE secret; only the numeric-reference assignment receives E5 paths.
The CE REST, MCP and CLI public scenarios also execute with instrumentation and the ordinary report guard.
The workflow owns PostgreSQL, Redis and pinned reference assets because the complete plan needs those resources.
It does not require sea-orm-cli or an extra outer migration.
`cargo llvm-cov show-env --sh --branch` instruments the external processes, then `report` exports coverage without a second test run.
The uploaded artifacts retain exact receipts, redacted failure logs, module CSV/JSON, branch counts and the lowest-covered module report.
The private live-provider contract remains explicitly opt-in and is not counted as covered execution.
This workflow is verified on PRs changing its implementation as well as on its weekly schedule.
