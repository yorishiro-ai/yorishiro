# Loco / SeaORM architecture audit

調査日: 2026-10-07

## 結論

- 採用されている基本方針は妥当です。
  generated Entity を `src/models/_entities/` に置き、手書きの table extension で ActiveModel の hook、finders、table-owned persistence を補い、controller/worker/task はモデル API を呼ぶ構成です。
- repository 層は実装されていません。
  `rg` で `Repository` / `repository` を確認したところ、実装・trait・依存はなく、主な出現は architecture 文書と migration の旧コメントだけでした。
- SeaORM 2.0.4 / SeaQuery 1.0.2 は root と migration の双方で利用されています。
  `Cargo.toml:28-33,53-59`、`Cargo.lock:5440-5490`。
- Entity API は通常の CRUD、filter、join、projection、bulk update/delete、upsert、transaction に十分活用されています。
  したがって「全面的に raw SQL へ寄せる」「repository 層を追加する」は不採用です。
- 手書き SQL は主に、RLS/session GUC、advisory lock、pgvector/sqlite-vec、FTS5/pg_trgm、trigger/GRANT/role、atomic `INSERT ... SELECT`、複雑な cross-plane projection に限定されています。
  これらは SeaORM/SeaQuery の portability・表現力・権限要件を超えるため、現時点では多くが正当です。
- 最も注意すべき設計リスクは、raw SQL の parameter placeholder と backend 分岐、動的 table name の allowlist、identity pool と tenant transaction の取り違えです。
  現状はコメントと allowlist で防御していますが、各 raw query に回帰テストを維持する必要があります。

## 調査範囲と基準

確認対象は `src/`、`ee/`、`migration/`、`tests/`、`Cargo.toml`、`Cargo.lock` です。
generated entity は仕様確認に使い、編集対象とはみなしません。

採用基準:

1. 通常の table CRUD、条件、sort、join、projection、bulk mutation は Entity API / SeaQuery builder を優先する。
2. ActiveModel の lifecycle、入力検証、table-owned invariant は各 table extension に置く。
3. 複数 write の原子性、RLS scope、advisory lock、競合防止が必要なら transaction をモデル API の引数で明示する。
4. raw SQL は SeaORM に相当する表現がない、または一文の atomicity/performance/engine-specific semantics が必要な場合だけ許可する。
5. raw SQL の table name は固定値または allowlist 由来に限定し、値は bind parameter にする。
6. migration は SeaQuery builder を標準とし、backend 固有 DDL・role/RLS/trigger・型拡張だけ `execute_unprepared` / `Expr::cust` を使う。
7. 新しいライブラリは既存の SeaORM/SeaQuery/sqlx/標準 API で代替できないこと、または明確な backend/性能要件があることを示せない限り追加しない。

不採用基準:

- query を一箇所に集めるだけの repository facade。
- model/table ownership を跨ぐ汎用 repository、DAO、service 層。
- SeaQuery builder で表現できる query の raw SQL 化。
- RLS scope と pool selection を隠す抽象化。
- backend 差を無理に一つの SQL に押し込み、SQLite/PostgreSQL の制約や型差を隠す helper。

## 依存関係

### SeaORM / SeaQuery / sqlx

- SeaORM は workspace dependency として `sqlx-sqlite`、`sqlx-postgres`、`runtime-tokio-rustls`、`macros`、`postgres-vector` を有効化している。
  `Cargo.toml:52-60`。
- migration crate は `sea-orm-migration` 2.0.4 の `sqlx-postgres` / `sqlx-sqlite` を使用する。
  `Cargo.toml:28-33`。
- `sqlx` 0.9.0 は直接 dependency で、SeaORM の通常 query の置き換えではなく、pool lifecycle、`SET ROLE`、GUC、`PgConnection` の fire-and-forget update に使われています。
  `Cargo.toml:50,75-78`、`src/db.rs:1-18,132-156,201-216`、`src/models/api_keys.rs:156-171`。
- `libsqlite3-sys` の直接依存は sqlite-vec の auto-extension registration のためです。
  SeaORM/sqlx が同じ FFI をそのまま再 export しないという理由が `Cargo.toml:98-102` と `src/db.rs:95-113` に明記されています。
- `regex`、`jsonschema`、embedding runtime（candle/tokenizers）、HTTP (`reqwest`) などは SeaORM と無関係の明確な利用があります。
  `Cargo.toml:71,84,90-98`、`src/models/schema_schemas/metaschema/validate.rs:1-40`、`src/services/embedding/local.rs:1-9`。
  SeaORM 置換候補ではありません。
- `Cargo.lock` の `sea-orm` 周辺には `sea-query`、`sea-query-sqlx`、`sea-schema`、`sea-orm-macros` が解決されています。
  依存重複を理由に別 ORM・query builder を追加する根拠はありません。

## Entity / ActiveModel / table ownership

- `src/models/mod.rs:1-45` は一 table 一 module の構造を明示しています。
  generated entities は `_entities`、application-owned extension は同じ table 名の module です。
- `src/models/entity_entities.rs:9-16,39-44` は大きな model の実装を `crud`、`migration`、`snapshots`、`validation` に分割しており、model ownership を維持したまま可読性を確保しています。
- ActiveModel hook は PostgreSQL の DB default と SQLite の差を吸収しています。
  例: `src/models/entity_entities.rs:145-155`、`src/models/workspace_workspaces.rs:24-34`、`src/models/api_keys.rs:76-85`。
  `sqlite_generated_id` と `stamped_updated_at` の共通化は重複 helper ではなく、同じ cross-backend invariant の妥当な共通化です。
  `src/db.rs:229-260`。
- generated entity には `DeriveRelation` と `Related` が生成され、workspace/tenant/schema/user の FK・has-many が定義されています。
  例: `src/models/_entities/workspace_workspaces.rs:28-160`、`src/models/_entities/entity_entities.rs:27-70`。
- relation API も実際に利用されています。
  `src/models/workspace_workspaces.rs:67-78` は `left_join`、`src/models/tenant_memberships/operations.rs:57` は `find_also_related`、`src/models/entity_entities/migration.rs:88-100,209-216` は relation definition による join です。
- 多くの domain operation は generated Model を直接外部へ漏らさず、`EntityRecord`、`WorkspaceRecord` 等の application DTO/model record に変換しています。
  `src/models/entity_entities.rs:167-190`、`src/models/workspace_workspaces.rs:147-183`。
  これは transport DTO と DB Model の結合を避ける良い設計です。
- 空の `impl Model {}` / `impl ActiveModel {}` / `impl Entity {}` は generator layout の extension point として残っています。
  それ自体は不要な abstraction ではありませんが、実際の custom API が増えた table では extension impl に集約し、controller に query を戻さない判断を続けるべきです。

## Queries and SeaORM feature usage

### 十分に使われている機能

- `Entity::find`、`find_by_id`、`filter`、`one/all` は全 model に広く使用。
  例: `src/models/entity_entities/crud.rs:53-126`、`src/models/query_embedding_requests.rs:97-226`。
- projection は `select_only`、`column`、`column_as`、`into_model`、`into_tuple` で明示的に使われています。
  `src/models/entity_entities.rs:96-116`、`src/models/workspace_workspaces.rs:67-78`、`ee/models/marketplace.rs:339-370,427-454`。
- `update_many().col_expr(...)` は conditional update、counter、timestamp、optimistic guard に使われています。
  `ee/models/inference_jobs.rs:117-129,168-206`、`src/models/entity_entities.rs:27-37`、`src/models/workspace_workspaces.rs:130-143`。
  これは `ActiveModel` 一行更新より atomic で、raw SQL にする必要がありません。
- `delete_many` は scoped delete に利用されています。
  `src/models/api_keys.rs:330-353`、`src/models/entity_relations.rs:265`、`src/models/query_embedding_requests.rs:202-225`。
- `OnConflict` による upsert/do-nothing は複数箇所で利用されています。
  `ee/models/workspace_worker_classes.rs:52-70`、`ee/models/workspace_llm_keys.rs:75-111`、`ee/models/tenant_billing.rs:79-120`、`src/models/tenant_memberships/operations.rs:25-38`。
- `TransactionTrait`、`DatabaseTransaction`、commit/rollback は競合する domain operation ごとに明示されています。
  `src/workers/query_embedding.rs:222-274`、`src/tasks/create_workspace.rs:34-39`、`ee/models/marketplace/write.rs:33-41`、`ee/models/inference_jobs.rs:152-206`。
- pagination/paginator も利用されています。
  `ee/models/tenant_tenants.rs:6`、`ee/models/marketplace.rs:471-480`、`ee/tasks/sqlite_ann_benchmark.rs:35-38`。

### 利用できるが、現状で無理に導入しない機能

- relation eager loading (`find_with_related`) は限定的です。
  generated relation は豊富ですが、RLS と control-plane/tenant pool の分離があるため、単純な eager load を横断的に導入すると pool境界・権限境界を壊しやすいです。
  まず join/projection で必要列だけを取得し、複数 table が同じ connection/transaction で安全に読めることを確認できる場合だけ採用します。
- `insert_many` は `Entity::insert` と `OnConflict` より使用例が少ないです。
  SQLite の `before_save` が呼ばれない insert path が存在するため、bulk insert 導入時は ID/default/hook の挙動を検証する必要があります。
  既存の注意書きは `src/models/tenant_memberships/operations.rs:25-38` にあります。
- stream/cursor は大量 export/reindex の候補ですが、現在は batch/paginator と `get_batch_with_tokens` が使われています。
  `src/models/entity_entities.rs:77-94`。
  導入判断は memory profile と transaction lifetime を測定してからにします。
- SeaORM `QueryTrait`/SeaQuery の DB-specific expression はすでに一部利用されています。
  JSONB containment は raw SQL ではなく `PgExpr::contains` を使う実装があります。
  `src/models/entity_entities/crud.rs:216-244`。
  これを一般化するより、SQLite unsupported を明示する現在の設計の方が安全です。

## Raw SQL audit

### 正当性が高いもの（採用）

- RLS/session state:
  `src/db.rs:170-190,201-216` は `set_config`、`SET ROLE`、`RESET` が必要です。
  `src/db.rs:177-178` の説明どおり SeaORM に non-SELECT session configuration API がないため、sqlx/Statement は妥当です。
- advisory locks:
  `src/db.rs:304-336,404-416` は PostgreSQL 固有 primitive で、SeaORM の通常 Entity API では代替できません。
- pgvector/sqlite-vec/vector table:
  `src/models/search.rs:119-185,315-419` は `<=>`、`vec_distance_cosine`、FTS5 `MATCH/bm25`、pg_trgm `similarity/%` を使うため raw SQL が必要です。
  table name は `src/models/search.rs:305-311` で embedding table allowlist を検証しています。
- portable anti-join across width tables:
  `src/models/entity_entities.rs:119-142` は runtime の width table union を組み立てる必要があり、`INSERT ... SELECT`/anti-join の semantics も明確です。
- atomic `INSERT ... SELECT` snapshot:
  `src/models/entity_entities/backend.rs:4-28` は race semantics を維持する一文の write であり、Entity API への分解は不採用です。
- atomic `COALESCE` update:
  `src/models/workspace_workspaces.rs:212-237` は concurrent schema creation の TOCTOU を防ぎ、column-level GRANT も満たします。
  ただし現行コメントの「SeaORM cannot express COALESCE in a Set clause」は、SeaQuery の expression API 全体ではなく、ここで必要な ActiveModel/column subset 制約として読むべきです。
- cross-plane computed projection:
  `ee/models/schema_schemas/query.rs:42-69`、`ee/models/marketplace.rs:284-312,382-399` は control-plane と workspace table を join し、computed summary を返すため raw SQL の理由が明確です。
- authentication function / best-effort timestamp:
  `src/models/api_keys.rs:96-126,156-171` は SECURITY DEFINER function と bare connection の意図があり、SeaORM 化すると RLS bypass または read-only transaction rollback の意味を失います。
- migration DDL not covered by SeaQuery:
  RLS/GRANT/role/trigger/custom type/GIN/HNSW/array は `migration/src/helpers.rs:73-176` と `migration/src/m20260829_000000_initial_schema.rs:536-570,807-945` に理由があります。

### 要監視・改善候補

- `ee/models/schema_schemas/query.rs:48-66` は backend を `Postgres` に固定しています。
  呼び出し側も enterprise の PostgreSQL-only surface として扱う必要があり、SQLite で到達可能なら明示的な backend guard を追加するのが望ましいです。
  `ee/app.rs:24-28` は enterprise feature の SQLite limitation を説明していますが、query 自体にも同じ契約を近接させると誤用が減ります。
- raw query の placeholder は PostgreSQL `$N` と SQLite `?` が混在します。
  `src/models/search.rs:144-184,370-385` は backend ごとに分けていて妥当ですが、共通 helper にまとめる際に placeholder を共有しないことが重要です。
- `src/db.rs:72-79` と `ee/controllers/middleware/auth.rs:66` は sqlx の direct query です。
  ORM に寄せる理由はなく、どちらも backend/system function を読む専用 query として残す判断です。
- `src/models/entity_entities/backend.rs:12-24`、`src/models/workspace_workspaces.rs:224-233` などの SQL は現在 bind 値です。
  新しい query も文字列 interpolation を値に使わないことをレビュー基準にします。
- migration の raw SQL helper は `format!` で table/policy/column 名を組み立てます。
  全 caller が hardcoded migration constants であることが前提です。
  caller が user/config 入力を受ける helper へ拡張しないことが採用条件です。

## Migrations

- migration は `sea_orm_migration::prelude::*` と SeaQuery builder を標準利用しています。
  最新 migration も Table/Index/alter/ForeignKey builder です。
  例: `migration/src/m20261006_000018_query_embedding_requests.rs:27-96`、`migration/src/m20261007_000019_active_reindex_admission.rs:1-45`。
- 全 migration は `helpers::use_transaction()` を override しています。
  `migration/src/helpers.rs:10-20` の SQLite DDL interleaving 防止理由は合理的です。
- backend bridging helper は UUID、JSON、timestamp、checks、RLS、GRANT を集約しています。
  `migration/src/helpers.rs:23-70,115-230`。
  PostgreSQL/SQLite の型差を各 migration が個別に再実装しない点は重複削減として評価できます。
- `initial_schema` が大きく、backend-specific DDL が同一ファイルに集中しています。
  `migration/src/m20260829_000000_initial_schema.rs:68-1914`。
  ただし migration の履歴を改変せず、今後の migration で分割するのは schema history を壊すため不採用です。
- generated entities と migration の table names は一致し、`src/models/_entities/` は自動生成物です。
  AGENTS のルールどおり generated file を手修正しない判断を維持します。
- 配置上の不一致として、`migration/src/m20260829_000000_initial_schema.rs:572,1439` の「repository layer」というコメントは現行の table-owned model architecture と用語がずれています。
  実装を変える必要はありませんが、将来コメントを直すなら「identity/control-plane model path」など実際の pool/model path を示す表現が適切です。

## Transactions and pool boundaries

- `TenantDb::begin_for_workspace` が SeaORM `DatabaseTransaction` を返し、その transaction 上で Entity API と raw SQL を同じ scope に載せています。
  `src/db.rs:163-192`。
  これは RLS の transaction-local GUC と domain write の atomicity を同時に満たす中心設計です。
- request handler が transaction を明示的に commit する契約は `src/db.rs:163-167` に記録されています。
  read-only request の drop rollback と `last_used_at` の別 connection 更新の理由も `src/db.rs:264-275`、`src/models/api_keys.rs:156-171` にあります。
- signup/provisioning は `&DatabaseTransaction` を受けることで、user/tenant/workspace/membership を一緒に commit する契約を型で強制しています。
  `ee/models/user_users/provisioning.rs:35-44,82-128`。
- SQLite は RLS/role がなく single-tenant 制約を持つため、PostgreSQL の Authenticator/tenant pool と同じ abstraction を無理に共有していません。
  `src/models/tenant_tenants/operations.rs:11-50`、`src/controllers/middleware/auth.rs:182-185`。
- transaction を model API の引数にする現在の方式は repository 層より安全です。
  repository を追加すると `DatabaseConnection` と `DatabaseTransaction` の混同、RLS context の消失、commit ownership の隠蔽が起きやすいです。

## 重複 helper / placement / naming

- 正当な共通 helper:
  `migration/src/helpers.rs` の backend DDL helper、`src/db.rs` の ID/timestamp/advisory-lock helper、`src/models/entity_embeddings.rs` の width table registry。
  いずれも backend invariant または security invariant があり、単なる短縮ではありません。
- 重複リスク:
  table module の `before_save` は各 entity に必要ですが、ID/timestamp の判定ロジックを各所へコピーせず `src/db.rs:229-260` を使い続けるべきです。
  `OnConflict` と update_many の scoped condition も table-owned operation に置き、generic repository helper にまとめない判断です。
- 配置は概ねルールに一致します。
  外部 provider は `src/services/embedding`、table logic は `src/models`、REST/MCP は `src/controllers`、migration は `migration/src`、enterprise overlay は `ee/` にあります。
- `ee/models/schema_schemas/query.rs` は base table を読むものの enterprise endpoint のため `ee/` にある、という説明が明確です。
  `ee/models/schema_schemas/query.rs:3-8`。
- `src/models/export.rs` / `import.rs` / `recall.rs` は cross-table facade ですが repository ではありません。
  それぞれ export/import/read composition の domain operation で、SQL ownership は table model に残っています。
- `src/models/_entities` は generated naming、handwritten extension は `<table>.rs` naming で整合しています。
  新しい table でもこの規約を優先します。

## 不要な外部ライブラリ候補

SeaORM/DB 設計に限定して明確な不要依存は確認できませんでした。

- `sqlx` は direct pool hooks、role/GUC、system views、bare connection update に使用され、削除不可。
  `src/db.rs:1-18,67-79,132-156`、`src/models/api_keys.rs:156-171`。
- `libsqlite3-sys` は sqlite-vec extension registration の FFI に使用され、削除不可。
  `src/db.rs:95-113`。
- `sqlite-vec` は SQLite vector search に使用され、削除不可。
  `src/db.rs:15,81-113`、`src/models/search.rs:166-184`。
- `sea-orm-migration` は boot-time migration と migration crate に必要。
- `regex`、`jsonschema`、`candle-*`、`tokenizers`、`reqwest`、`rmcp` 等は DB abstraction の重複ではなく、各機能で直接使用されています。
- `anyhow` は `YorishiroError::Internal` の内部原因構築に使われていますが、DB abstraction 置換を理由に削除できる状況ではありません。

依存削減の候補を作る場合は、まず `cargo tree -i <crate>` と feature 別 build (`--no-default-features --features community`) を確認し、enterprise-only direct dependency を root optional dependency のまま維持できるかを検証します。
現時点で SeaORM のためだけに導入された重複 ORM/query library はありません。

## 推奨アクション

1. repository 層は追加しない。
   既存の table extension と cross-table domain facade を維持する。
2. raw SQL を新設する場合、理由、backend、transaction/pool、bind strategy、dynamic identifier allowlist、対応テストを同じ module に記載する。
3. `ee/models/schema_schemas/query.rs` の PostgreSQL 固定 query は、SQLite での到達不能契約を test または明示 guard で固定する。
4. migration の「repository layer」コメントを、現行の identity/control-plane model path と一致する用語へ将来更新する。
5. vector/search/trigger/RLS SQL は SeaORM builder へ機械的に置換しない。
   代替可能性があるのは、raw SQL の普通の filter/join/projection 部分だけであり、性能と backend parity の test を先に要求する。
6. 新しい bulk insert / eager relation / streaming を導入する場合、ActiveModel hook、RLS transaction、SQLite UUID BLOB、memory/connection lifetime を検証する integration test を先に追加する。

## 監査判断

現行コードは SeaORM を避けているのではなく、通常操作では Entity API を使い、ORM が表現できない DB/security/vector semantics にだけ raw SQL を使う設計です。
repository 層を作ると、既に table module が持つ ownership と transaction/pool boundary を二重化するため、不採用とします。
主な残存リスクは raw SQL の backend 固定・動的 identifier・migration の hardcoded SQL が将来の変更で壊れることです。
それを防ぐ最小策は、新しい抽象層ではなく、既存の allowlist、近接コメント、PostgreSQL/SQLite integration tests、`make check-all` / `make check-all-ee` の継続です。
