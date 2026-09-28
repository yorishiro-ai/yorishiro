# モデルの配置

このページは Issue #472 の第 1 フェーズを説明します。

## 生成コードとの境界

`migration/src/` のマイグレーションは手書きでデータベーススキーマを定義します。
`cargo loco db entities` は SeaORM エンティティを `src/models/_entities/` 以下に生成します。
生成ディレクトリはその場所に残し、手で編集してはいけません。
Loco 1.1.0 のジェネレーターは出力先 `src/models/_entities` を固定しています。
さらに `src/models/mod.rs` を読み、ルートに手書き拡張がなければそこへ作成します。
そのため、ルートのモデルファイルは互換パスとジェネレーター用のマーカーとして残し、実装ソースは機能領域に置きます。
コントローラー、タスク、ワーカー、手書きモデルロジックはアプリケーションコードであり、生成対象ではありません。
各領域の `mod.rs` は配置を示すためだけに存在します。
ルートの `#[path]` 宣言は、Loco 1.1.0 の制約と既存の呼び出し側パスを両立するためのコンパイル用ブリッジです。
未使用の幅別 embedding 拡張はルートのゼロバイトの生成マーカーとして残し、意図的にコンパイルしません。
新しいテーブルを追加すると、Loco は `src/models/` のルートに拡張ファイルを書き出します。
生成後にその実装を適切な領域へ移動し、ルートに互換用の `#[path]` 宣言を追加してください。

チェックイン済みの `Makefile` の `entities` ターゲットは、使い捨て PostgreSQL にマイグレーションを適用して `cargo loco db entities` を実行します。
CI は 10 個以上のエンティティファイルが生成されることと、生成結果がチェックイン済みツリーと一致することを検証します。
3 つの pgvector エンティティファイルは必要な `ActiveModelBehavior` 実装を SeaORM codegen が省略するため、比較前に復元します。
2026-09-28 の直接実行では、`src/models/_entities` 以下に 31 ファイルが生成されました。
文書化された 3 つの pgvector 例外を復元した後、その他の生成ファイルはすべてバイト単位で一致しました。

## 機能領域マップ

移動した手書きモデルモジュールはすべて 1 つの領域に分類しています。
`pagination.rs` は特定のテーブルに属さない共有モデル基盤のため、意図的にモデルルートへ残しています。
ルートのマーカーファイルは既存の `crate::models::<module>` パスを呼び出し側のために維持します。

| 領域 | 移動したモジュール | 新しい実装パス |
| --- | --- | --- |
| Content | `entity_column_preferences`、`entity_embeddings`、`entity_embeddings_1024`、`entity_embeddings_1536`、`entity_embeddings_768`、`entity_entities`、`entity_relations`、`entity_snapshots`、`export`、`import`、`recall`、`schema_schemas`、`search`、`workspace_schema_fork_heads`、`workspace_schema_forks` | `src/models/content/` |
| Identity | `api_key_audit_log`、`api_keys`、`tenancy`、`tenant_memberships`、`tenant_tenants`、`user_users`、`workspace_embedding_keys`、`workspace_invites`、`workspace_llm_keys`、`workspace_workspaces` | `src/models/identity/` |
| Templates | `template_reviews`、`template_templates`、`template_versions` | `src/models/templates/` |
| System | `compute_credit_ledger`、`inference_jobs`、`stripe_events`、`system_maintenance`、`tenant_billing`、`tenant_reindex_schedules`、`workspace_worker_classes` | `src/models/system/` |

意図的にルートへ残すモデルモジュールは `pagination.rs` と `src/models/` の互換マーカーファイルです。
`_entities/` も意図的にルートへ残しますが、これは生成コードであり機能領域の手書き実装ではありません。
`src/models/content/entity_entities/` は Content モデルの実装詳細であり、1 テーブル 1 モデルという単位は維持しています。
