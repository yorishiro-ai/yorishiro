# 時計に依存する判定

この一覧はアプリケーション内の実時間の利用箇所と、ローカルな時刻 seam を導入した判定箇所および見送った理由を示します。

## 導入した seam

| 箇所 | 判定 | seam と境界テスト |
|---|---|---|
| `ee/services/oauth/state_token.rs` | OAuth `state` の発行時刻と有効性 | `issue_at` と `verify_at` が時刻をローカルに受け取り、600 秒の有効期限境界と 5 秒の未来時刻許容境界をテストします。 |
| `ee/controllers/stripe.rs` | Stripe 署名の timestamp 許容範囲 | 既存の `verify_at` が受信検証時刻を受け取り、300 秒の両端を含む境界と範囲外の両方をテストします。 |
| `ee/controllers/middleware/edition.rs` | リクエストごとのライセンス期限 | 既存の `is_active_at` が Unix 秒を受け取り、`exp > now` の排他的な境界をテストします。 |
| `src/models/workspace_invites/operations.rs` | 招待の発行と redemption の期限 | 公開された `create_invite_at` と `redeem_invite_at` が 1 つの UTC 値を受け取るため、統合テストに feature フラグは要りません。backend テストで `expires_at == now` は期限切れ、期限直前は有効、期限直後は期限切れとなることと TTL 計算を確認します。 |
| `ee/tasks/reindex_scheduler.rs` | リインデックス予定時刻の到来判定 | `run_at` が tick を受け取り、`is_due` は `scheduled_for <= tick` を使います。-301、0、+1、+300、+301 秒をテストします。スケジュール自身の間隔が次回実行までの間隔であり、未実行分の猶予ではありません。 |

本番の wrapper は `chrono::Utc::now()` を使います。既存の時間幅、レスポンス、ログ、ライセンスゲート、キュー処理、永続化処理は変更しません。

## 棚卸しと見送り

| 呼び出し箇所 | 分類 | 見送った理由 |
|---|---|---|
| `src/db.rs`、`src/models/compute_credit_ledger.rs`、`schema_schemas.rs`、`system_maintenance.rs`、`tenant_reindex_schedules.rs`、`workspace_schema_forks.rs`、`ee/models/billing.rs`、`compute_credit_ledger.rs`、`embedding_keys.rs`、`entity_columns.rs`、`inference_jobs.rs`、`llm_keys.rs`、`marketplace.rs`、`schema_schemas.rs`、`template_templates.rs`、`worker_classes.rs`、`workspace_schema_forks.rs`、`src/models/api_keys.rs`、`migration/src/helpers.rs`、`migration/src/m20260829_000000_initial_schema/schema.rs` | `created_at`、`updated_at`、`last_used_at`、origin 時刻などの永続化 timestamp | データベースとモデルの timestamp は永続化の責務であり、外部から観測される期限判定ではありません。抽象化すると seam が広がり、トランザクションや backend の default の意味を変える危険があります。 |
| `src/controllers/middleware/access_log.rs`、`src/initializers/db_load_guard.rs`、`ee/tasks/sqlite_ann_benchmark.rs` | 単調時計による経過時間の計測 | これらは実時間の期限や予定を判定せず、実行時間だけを計測します。`Instant` が適切な時計であり、置換を必要とする決定テストもありません。 |
| `src/controllers/middleware/rate_limit.rs` | 単調時計による quota window | 経過時間で window を管理しており、単調時計を置き換える決定テストは現時点で必要ありません。 |
| `src/services/embedding/model_fetch.rs` | stale な partial download の filesystem mtime | 本番の判定は filesystem metadata に基づき、テストも mtime を直接設定します。時計 seam を追加しても外部動作のテストは改善しません。 |

プロセス全体の clock、service locator、データベース timestamp の抽象化、`Instant` の置換は導入していません。
