# Test assignment checks

Public contracts run the real binary through `tests/blackbox/run.py`.
Their reports are checked by `scripts/check_blackbox_reports.py` against the exact scenario list and executable hash.
Common public contracts run once on CE SQLite; enterprise composition and backend guarantees need their own explicit assignments, not another full cross-product.

The compiled inventory is a separate check.
GHA builds CE and EE workspace test targets with Cargo's JSON message format, locates the reported executables, and enumerates their actual libtest identities.
`scripts/test_assignments.py` joins both inventories to `assignments.json`.
New, missing, stale or ambiguous assignments must fail rather than silently select zero tests.
An inventory receipt proves classification, not execution of the classified test.

This additive stage deliberately keeps the legacy matrix.
Mixed common/backend tests must be split and assignment-driven execution verified before that matrix is removed.
Known resource-dependent tests and subprocess helpers must remain visible in the inventory rather than disappear through a broad ignore rule.

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
