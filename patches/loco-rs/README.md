# Loco 1.3.1 upstream bug fixes

## Provenance and application

The verifier generates the Git-ignored `source/` directory from the `loco-rs` 1.3.1 registry archive and the diffs under `patches/`.
Each diff mirrors its upstream path, for example `patches/src/bgworker/pg.rs.patch`.
Run `python3 patches/loco-rs/verify.py --prepare` before direct Cargo commands on a fresh checkout.
Make build/check targets, CI toolchain setup and Docker builds restore it automatically.
The archive SHA-256 matches the registry checksum: `8aa30671d3a487b08a61cec5e9d9b9764cdf73e7df074fd1618811935214e076`.
The crate's upstream revision is `3ddc279740ebf0516403ca8f7028ce481d616a02` in [loco-rs/loco](https://github.com/loco-rs/loco).
Its Apache-2.0 licence remains in `source/LICENSE`.
The upstream files, including their original whitespace and `Cargo.lock`, are retained byte for byte except for the documented diffs.
The `.gitattributes` whitespace exception preserves the unified diff's required context prefixes; the verifier still compares every byte.
`source/Cargo.lock` is archival upstream material, not the application's resolution; builds and audits use the root `Cargo.lock`.
The root `[patch.crates-io]` selects this source for both editions.

Run `python3 patches/loco-rs/verify.py` to check the pinned archive, apply the diff with no fuzz in a temporary directory, and compare every source file.
The verifier downloads the archive only if it is absent from Cargo's cache.
After an intentional source edit, regenerate the per-file diffs with `python3 patches/loco-rs/verify.py --refresh`, review them, then run the verifier without `--refresh`.
Changing the pinned version requires reviewing these fixes against the new upstream source, not blindly applying the old diff.

## Queue fixes

| ID | Bug in upstream 1.3.1 | Fix | Regression check |
|---|---|---|---|
| LQ-01 | Redis single-job enqueue writes the payload and queue membership separately. | Write both in one Redis transaction. | Enqueue and dispatch tests and Redis topology suites. |
| LQ-02 | Claiming a job does not refresh its processing timestamp, so the reaper can redeliver it immediately. | Atomically claim the unchanged candidate, record `processing` and the claim time, and assign an attempt token. | `redis_claim_refreshes_age_and_fences_reaped_completion_and_failure`. |
| LQ-03 | A reaper or stale worker can overwrite a newer job state. | Atomically compare the record and set membership on reap, and the attempt token on completion or failure. | The LQ-02 test also checks stale success and failure across redelivery. |
| LQ-04 | Persistence errors on completion or failure log the whole job, including import bodies and credentials. | Log only the job ID. | Review the worker diffs. |
| LQ-05 | Queue shutdown stops polling without releasing SQL pools or handlers that hold `AppContext`. | Clear handlers and close SQL pools when `close()` runs after workers drain. | `sql_queue_close_releases_pool_and_retained_worker_context` and request teardown. |

`shutdown()` remains a stop signal.
Call `close().await` after `run()` drains the workers.
The force-quit signal bypasses graceful draining.
These fixes do not make job effects exactly-once: application transactions and idempotence remain necessary.
Do not mix patched and unpatched Redis workers during rollout: drain and replace the worker fleet because unpatched workers do not participate in attempt fencing.

## Removal condition

Replace this override with a published Loco release only when it contains equivalent atomic claim/reaper/completion fencing, payload-safe error logs, atomic enqueue and awaited resource cleanup.
Run the regression tests and the CE/EE queue topologies before deleting the override, source, patch and verifier together.
No upstream issue or accepted upstream fix is implied by these local defect IDs.
