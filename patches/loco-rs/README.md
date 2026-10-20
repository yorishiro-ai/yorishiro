# Loco 1.2.0 upstream bug fixes

## Provenance and application

`source/` is ignored generated material: the published `loco-rs` 1.2.0 crate with `fixes.patch` applied.
Only the patch, provenance and restoration script are tracked in Git.
Run `python3 patches/loco-rs/verify.py --prepare` before direct Cargo commands on a fresh checkout.
Make build/check targets, CI toolchain setup and Docker builds restore it automatically.
The archive SHA-256 is `5386baf405736a2620001f4561b00fa4874dae99eac3ccf01713fabc9ef13af4`, matching the registry checksum before the path override.
The crate's upstream revision is `43b3fe60db487758c195d6947f1c37e5246cedf2` in [loco-rs/loco](https://github.com/loco-rs/loco).
Its Apache-2.0 licence remains in `source/LICENSE`.
The upstream files, including their original whitespace and `Cargo.lock`, are retained byte for byte except for `fixes.patch`.
The `.gitattributes` whitespace exception preserves the unified diff's required context prefixes; the verifier still compares every byte.
`source/Cargo.lock` is archival upstream material, not the application's resolution; builds and audits use the root `Cargo.lock`.
The root `[patch.crates-io]` selects this source for both editions.
No Cargo registry cache, shared database, or running container is modified.

Run `python3 patches/loco-rs/verify.py` to check the pinned archive, apply the diff with no fuzz in a temporary directory, and compare every source file.
The verifier downloads the archive only if it is absent from Cargo's cache.
After an intentional source edit, regenerate `fixes.patch` with `python3 patches/loco-rs/verify.py --refresh`, review the diff, then run the verifier without `--refresh`.
Changing the pinned version requires reviewing these fixes against the new upstream source, not blindly applying the old diff.

## Defects corrected

| ID | Upstream bug | Fix | Regression check |
|---|---|---|---|
| LQ-01 | Redis single-job enqueue wrote the payload and queue membership separately. | Write both in a Redis transaction, as upstream batch enqueue already does. | Existing enqueue/dispatch tests and Valkey topology suites. |
| LQ-02 | Claiming a long-queued job did not refresh its processing timestamp, so the reaper could immediately redeliver it. | Atomically claim the unchanged candidate, persist `processing` and the claim time, and assign a unique attempt token. | `redis_claim_refreshes_age_and_fences_reaped_completion_and_failure`. |
| LQ-03 | A reaper could overwrite a job completed or reclaimed after its read; a stale worker could overwrite the replacement attempt. | Reaper transitions compare the exact record and set membership atomically; completion/failure compare the attempt token atomically. | The same regression test holds two real workers across redelivery and checks both stale success and failure. |
| LQ-04 | Completion/failure persistence errors logged the entire job, including import bodies and credentials. | Log the job ID instead of `Debug`-formatting its payload. | Review `fixes.patch`; this changes diagnostic fields, not stored job arguments. |
| LQ-05 | Queue shutdown only cancelled polling; SQL pools and handlers retaining `AppContext` were not released. | Add awaited `close()`, clear retained handlers and close SQL pools; boot calls it after workers drain. | `sql_queue_close_releases_pool_and_retained_worker_context` and real request teardown. |

`shutdown()` remains a stop signal.
Call `close().await` after the `run()` future has drained; do not close a SQL pool underneath active jobs.
The force-quit signal intentionally still bypasses graceful draining.
These fixes do not make job effects exactly-once: application transactions and idempotence remain necessary.
They do not prove or fix the separate, unexplained PostgreSQL authentication/cleanup delays seen in previous tests.
Do not mix patched and unpatched Redis workers during rollout: drain and replace the worker fleet because unpatched workers do not participate in attempt fencing.

## Removal condition

Replace this override with a published Loco release only when it contains equivalent atomic claim/reaper/completion fencing, payload-safe error logs, atomic enqueue and awaited resource cleanup.
Run the regression tests and the CE/EE queue topologies before deleting the override, source, patch and verifier together.
No upstream issue or accepted upstream fix is implied by these local defect IDs.

## 日本語

これはアプリ機能の追加ではなく、公開済みLoco 1.2.0のキューに対する上流バグ修正です。
元ソースとライセンスを保持し、変更差分を `fixes.patch` に分離しています。
`source/` はGit管理せず、`python3 patches/loco-rs/verify.py --prepare` で復元します。
元の空白と `Cargo.lock` も復元し、差分ファイルだけを空白検査の例外にします。
`source/Cargo.lock` は上流配布物の記録であり、アプリのビルドと監査にはルートの `Cargo.lock` を使います。
`verify.py` は配布アーカイブのハッシュ、パッチ適用結果、同梱ソースの一致を検証します。
LQ-01は単一投入の途中失敗、LQ-02は取得直後の誤った再配信、LQ-03はreaperと古いworkerによる状態の上書き、LQ-04はログへのpayload露出、LQ-05は終了時のプールとworker contextの保持を修正します。
回帰試験では実際のproviderとValkeyを使い、古い試行の成功・失敗が新しい試行を上書きしないことを確認します。
`shutdown()` は停止通知のままで、workerの終了後に `close().await` でリソース解放を待ちます。
稼働中workerのプールを先に閉じてはいけません。
旧workerは試行トークンを扱わないため、新旧workerを混在させず、処理をdrainしてから置き換えます。
過去に観測したPostgreSQLの認証・cleanup遅延との因果関係は未確定であり、その障害まで解消したとは扱いません。
上流の公開リリースに同等の修正が入り、回帰試験とCE/EEの各キュー構成が成功したときに、overrideとこのフォルダをまとめて削除します。
