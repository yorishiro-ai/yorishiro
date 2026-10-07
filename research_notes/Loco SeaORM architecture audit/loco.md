# Loco 1.2.0 / SeaORM architecture audit

調査日: 2026-10-07
対象: `loco-rs = 1.2.0`, `sea-orm = 2.0.4`, `sea-orm-migration = 2.0.4`

## 1. 結論

- 全体の骨格は Loco の推奨形に近い。Composition root は `edition/app.rs` の唯一の `Hooks` 実装、ルート・worker・task・initializer は Hooks から登録され、生成 entity は `src/models/_entities/` に隔離されている。
- `controllers` は入口、`models` は SeaORM entity 拡張とドメイン操作、`workers` は durable queue、`tasks` は CLI/定期実行、`initializers` はプロセス寿命の監視、`config/*.yaml` は Loco Config としておおむね適切に分離されている。
- 最大の意図的な拡張は RLS のための二重 DB 接続である。Loco の標準 `AppContext.db`/SeaORM 接続だけでは、接続取得・返却時の PostgreSQL session state と identity/tenant pool の分離を表現できないため、`sqlx` pool を `shared_store` に置いている。これは Loco/SeaORM の重複実装というより不足 API を補う境界である。
- 明確な配置違反は確認できない。ただし `.agents/skills/loco/doctrine.md` の標準配置には `views/`、`mailers/` がある一方、このアプリは API DTO を `src/dtos/` に置き、mail を一切送らない。これは API 専用・未使用機能という説明可能な差分であり、直ちに移動や依存追加は不要。
- 既存の Loco 機能で未使用なのは `ctx.mailer`、`ctx.storage`、`ctx.cache`、標準 JWT/auth、標準 mailer の実装である。未使用だから問題というより、同等機能を独自に再実装していないかを今後監視すべき対象である。
- 新規依存の追加は不要。Loco には queue/scheduler/task/initializer/testing、SeaORM には通常の DB API が既にあり、追加するなら標準機能を使えない具体的な制約を先に記録すること。

## 2. Loco 1.2.0 の設計契約と本リポジトリの対応

Loco skill の `doctrine.md`、`starter-app.md`、`recipes/testing.md` と pinned source `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/loco-rs-1.2.0/src/app.rs` を照合した。

### AppContext / Hooks

Loco の `AppContext` は `db`, `config`, `mailer`, `storage`, `cache`, `queue_provider`, `shared_store`, `environment` を持つ（依存ソース `app.rs:256-274`）。`Hooks` は `load_config`, `boot`, `initializers`, `middlewares`, `routes`, `after_context`, `after_routes`, `connect_workers`, `register_tasks`, `seed`, `on_shutdown` 等をアプリの composition point として提供する（依存ソース `app.rs:422-601`）。

- 実装の composition root は `edition/app.rs:38-107` の `impl Hooks for App` だけである。これは Loco の「アプリ wiring を Hooks に集約する」設計と一致する。
- `src/app.rs:39-56` は config 読み込みと `create_app::<H, Migrator>` の薄い委譲、`src/app.rs:59-72` は initializer/context の base wiring、`src/app.rs:335-375` は task/seed、`src/app.rs:305-314` は worker 接続を担当する。
- `src/lib.rs:8-12` は `edition` を composition root として re-export し、`src/` が edition を知らないという境界を守っている。
- `AppContext.shared_store` の利用は Loco の想定通り（依存ソース `app.rs:272-273`）。`src/app.rs:66-72`、`src/app.rs:391-415`、`ee/app.rs:39-57` で resolver、dispatcher、settings、pool を TypeId keyed DI として登録している。
- `AppContext` の標準 component を壊さない点も確認できる。アプリ側で `AppContext` を再構築している箇所は主にテストの `into_builder` 利用であり、Loco の builder が mailer/queue/cache/shared_store を引き継ぐ契約（依存ソース `app.rs:330-361`）に従うべきである。

### controllers / models / workers / tasks / initializers

- `src/controllers/` は route、extractor、HTTP/MCP rendering の入口である。`src/controllers/mod.rs:1-21` は Loco の module convention に沿う。`src/app.rs:116` 以降の `RouteMounts::community` で route group を明示登録しており、Rust の no-autoloading 問題を解決している。
- `src/models/` は table 単位の拡張で、`src/models/mod.rs:1-31` と `src/models/_entities/` の分離は Loco/SeaORM generator の契約に沿う。`_entities` は手編集しないこと。通常の finder/create/state transition は model/ActiveModel に置くという skill の fat-model 規約を今後も維持する。
- `src/workers/` は `BackgroundWorker` 実装で、`src/workers/mod.rs:1-9` の worker module と `src/app.rs:305-312` の registration が対応する。request を待たせない durable work は worker、単発/運用処理は task、という Loco doctrine の分類に合う。
- `src/tasks/` は `Task` の実装で、`src/app.rs:335-351` が一括登録する。Enterprise の recurring reindex は `ee/tasks/reindex_scheduler.rs` に Task としてあり、config の scheduler が task 名を起動するという Loco のモデルと合う。ただし production config では scheduler block がコメントアウトされている（`config/production.yaml:60-70`）。これは「未使用機能」ではなく、production で定期処理を無効にする設定である。
- `src/initializers/db_load_guard.rs:22-49` は Loco `Initializer` の `after_routes` を使ってプロセス内 monitor を開始し、`src/app.rs:59-61` で登録、`src/app.rs:354-356` で shutdown cleanup している。Loco source の initializer 契約（依存ソース `app.rs:604-635`）に沿う。なお `Initializer::check` は未実装なので `cargo loco doctor` でこの guard の設定/DB接続を検証しない。将来 health check が必要ならここに追加する。
- `src/data/config.rs` と `config/*.yaml` は Loco Config の入口として使われている。アプリ設定は `ctx.config.settings::<Settings>()`（`src/app.rs:391`）で取り、標準 doctrine の「設定は config、handler で ad-hoc env read をしない」に適合するのが CE 側の基本である。

## 3. 実際に存在するが未使用の Loco 機能

### 未使用または限定使用

1. **Mailer**: `AppContext.mailer` は Loco 標準の `Option<EmailSender>`（依存ソース `app.rs:267`）だが、`src/` に `mailers/` がなく、コード検索でも `ctx.mailer`/mailer send の利用はない。`config/development.yaml:157-178` は SMTP を有効化しているが、アプリは mail を送らない。`config/production.yaml:157-179` も opt-in block で、無駄な SMTP 接続を避けている。今後メール機能を追加する場合は `lettre` 等を追加せず `src/mailers/` と `ctx.mailer` を使う。
2. **Storage**: `ctx.storage` は Loco 標準（依存ソース `app.rs:268-270`）だが、アプリコードで利用していない。ファイル import/export があるが、`src/models/import.rs` や controller は DB/HTTP payload を扱う処理であり、file storage の独自再実装ではない。将来 blob を保存する際に S3 crate 等を直接導入せず、まず `ctx.storage` の driver を確認する。
3. **Cache**: `ctx.cache` は Loco 標準（依存ソース `app.rs:270-271`）だが未使用。`src/controllers/middleware/rate_limit.rs:19-24` は意図的な process-local `Mutex<HashMap>` の固定窓レート制限であり、一般的な cache の代替ではない。複数 replica で共有が必要な rate limit/cache を追加する場合は Loco cache/queue/DB の適否を先に評価する。
4. **標準 JWT/auth**: config の JWT は test/development に残るが、production は `config/production.yaml:180` 以降で intentionally absent。実装は API key 認証（`src/models/api_keys.rs:96-154`）であり、Loco 標準 JWT を使わない。二重の auth stack を追加しないこと。
5. **標準 `Hooks` optional hooks**: `init_logger`, `before_routes`, `before_run`, `dump` は override されていない（依存ソース `app.rs:493-529`, `548-550`, `579-595`）。これは未使用機能であって欠落ではない。custom router layer は `after_routes` に置かれている（`src/app.rs:237-267`）ので hook の選択も妥当。

## 4. Loco 標準との重複・独自実装の監査

### 意図的で妥当な独自実装

- **二重 pool / RLS session lifecycle**: `src/db.rs:1-11,52-64,123-156,163-180`。Loco の標準 `AppContext.db` は通常の SeaORM connection で、tenant/workspace を connection acquire/release と transaction-local `set_config` で扱う本アプリの RLS 要件を表さない。`sqlx` は `SET ROLE`、`after_connect`、`after_release`、advisory lock 等に必要で、単なる repository/ORM の重複ではない。`Cargo.toml:75-78` の理由付けも具体的で、新規依存追加は不要。
- **SeaORM raw SQL**: `src/models/api_keys.rs:100-115,156-171` の SECURITY DEFINER/authentication と last-used update、`src/db.rs:67-79,177-180` の PostgreSQL session state は、JSONB/pgvector/advisory lock/RLS のため標準 Entity API では表しにくい。通常 query を raw SQL に置き換える理由にはしないこと。
- **Loco queue の上にある durable domain lifecycle**: `src/models/queue_job_lifecycles.rs` と `src/workers/lifecycle.rs` は、Loco queue の retry だけでは足りない domain 状態、admission、heartbeat、reconcile を DB に記録している。Loco queue の代替ではなく、業務上の idempotency/observability layer として維持可能。ただし Loco の retry と二重にならないよう、状態遷移の責任範囲を文書化すること。
- **route inventory / MCP mount**: `src/app.rs:75-267` の inventory は OpenAPI/route completeness のアプリ固有要件。MCP `StreamableHttpService` を `after_routes` に mount する理由は `src/app.rs:230-236` にあり、Loco router registration の単純な重複ではない。rate limiter/maintenance guard も Axum 0.8 の layer 型制約を理由にしている。
- **sqlite-vec registration**: `src/db.rs:37-114` と `src/bin/main.rs:9-12`。SeaORM/Loco が sqlite-vec extension を自動登録する機能はないため、直接 FFI の `libsqlite3-sys` が必要。`Cargo.toml:98-102` の direct dependency は version mismatch を避けるためで、同機能の既存 Loco dependency が見つからない。

### 標準機能との境界を要注意とする箇所

- `src/initializers/db_load_guard.rs:44-46` は `tokio::spawn` するが、Loco skill が禁止する「request work を `tokio::spawn` で代用」するケースではない。initializer のプロセス寿命 monitor であり、shutdown の `abort`/await（同ファイル `55-60`）もある。今後 request 内の非同期仕事をここに逃がさず worker にする。
- `src/controllers/middleware/rate_limit.rs:19-24` の in-memory bucket は Loco cache の一般代替ではない。単一 process の auth/search guard という明示的要件に限定し、replica-aware rate limiting が必要になった時は Redis/Loco queue/cache/DB lock を比較する。
- `src/services/` は Loco doctrine が通常の arbitrary service/repository directory を禁じる一方、リポジトリの AGENTS.md は external clients only と定義している。現状は `src/services/embedding/` の外部 embedding client に限定されており、domain service/repository の配置違反は確認できない。
- controllers に DB query が存在する場合は、単なる finder/query を model に戻す。検索/ import/export/MCP の entry point は thin handler の範囲を超えやすいので、`src/controllers` で transaction/query assembly が増えたら `src/models/<table>.rs` または既存 table-owned private module に移す。

## 5. SeaORM 配置と生成物ルール

- `src/models/_entities/` は generated entity。`src/models/mod.rs:1-4` の説明どおり、generator の再実行で消える変更を入れない。
- table-owned extension は `src/models/<table>.rs` に置く。`src/models/api_keys.rs:91-185` の `impl ActiveModel`/`impl Entity` はこの形に沿う。
- migration は `migration/src/mYYYYMMDD_*.rs`、`migration/src/lib.rs` に登録する Loco/SeaORM migration convention を維持する。
- `src/dtos/` はこの API が wire DTO を明示的に分離するための application convention。Loco starter の `views/` と名前は異なるが、entity を直接 wire に出さないという目的は同じ。`src/dtos/system.rs:1-24` のように response shape を宣言し、entity serialize を避ける。
- repository layer は追加しない。finder/persistence は model extension に置く。横断的な external client だけ `src/services`、process wiring だけ initializer、composition だけ edition に置く。

## 6. 設定・scheduler・testing の注意点

- `config/development.yaml:47-53` は scheduler を有効にし、`tenant_reindex_scheduler` task を run_on_start する。一方 `config/production.yaml:64-70` は scheduler をコメントアウトしている。Enterprise reindex が production で必要なら、これは実行されない設定上の gap である。コードの scheduler 実装を worker の常駐 loop に置き換えるのではなく、Loco scheduler -> Task -> queue worker の三段を維持する。
- `config/development.yaml:158-169` の mailer host default `localhost` は、SMTP が不要な開発環境でも config の smtp block を生成する。現状 mailer 未使用なので connection failure/boot failure を招く可能性がある。production の opt-in conditional（`config/production.yaml:159-179`）の方が標準の `Option<EmailSender>` の扱いとして安全。メールを使わない間は development も block を無効化/conditional にするのが将来ルール候補だが、調査依頼の範囲では編集していない。
- Loco testing skill は `loco_rs::testing::prelude::*`、`request::<App,...>`、DB test の `#[serial]`、test config の `ForegroundBlocking` を推奨する。リポジトリには `tests/requests/`、`tests/models/`、`tests/workers/`、`tests/tasks/` があり、実際に Loco `AppContext` を組み立てている。queue lifecycle のような並行性試験では独自 queue runner を使うが、これは queue semantics を検証するためであり、通常 request test の harness を置き換えるルールにしない。
- test code に `std::env::var` は多い（例 `tests/requests/mod.rs:105-110`, `tests/workers/queue_routing.rs:84`）。skill の「application code は `ctx.config`、test/launcher の環境変数は許容」という線引きを明記しておく。`src/` の CE application code では env 読み出しを増やさない。既存 EE の `ee/data/oauth.rs:80` や `ee/controllers/middleware/edition.rs:93` は config への移行候補であり、AGENTS の edition boundary と機密値の扱いを含めて別 issue 化する。

## 7. 将来の作業ルール

1. 新しい DB table は migration -> `make entities` -> `src/models/<table>.rs` の順で追加し、`_entities` は手編集しない。
2. handler は auth/input/one or two model calls/render に限定する。普通の finder/query/transition は model に追加し、repository/service layer を作らない。
3. request を越える仕事は `BackgroundWorker`、人手/一回の運用処理は `Task`、定期起動は scheduler の Task entry。`tokio::spawn` は process-lifetime initializer/明示的な並行処理以外で使わない。
4. 設定可能な値は `config/*.yaml` + typed `Settings`/`ctx.config`。`src/` に新しい `std::env::var` を追加しない。EE 既存 env read は移行計画を持つ。
5. `ctx.mailer`、`ctx.storage`、`ctx.cache` を使う要件が出たら既存 Loco abstraction を先に採用し、同じ機能の crate や global singleton を追加しない。
6. `sqlx`/raw SQL は RLS session state、SECURITY DEFINER、JSONB containment、pgvector、advisory lock、single-statement invariant のように理由をコメントで残せる場合だけ使う。通常の read/write は SeaORM entity API。
7. `src/services` は外部 client のみ。domain logic は model、startup wiring は initializer/app、edition assembly は `edition/`。
8. Loco の route/worker/task wiring は生成・登録箇所を同時に確認し、変更後に `cargo loco routes`、`cargo loco scheduler --list`、対象 test を実行する。
9. Loco 1.2.0 の pinned API を `~/.cargo/registry/.../loco-rs-1.2.0/src` と skill の API index で確認してから実装する。推測で `loco_rs` symbol や queue API を追加しない。

## 8. 依存追加の評価

| 要求 | 追加依存 | 判定 |
|---|---|---|
| DB ORM/migration | Loco + SeaORM/SeaORM migration で充足 | 不要 |
| background queue/retry/scheduler/task | Loco 1.2.0 で充足 | 不要 |
| mail/storage/cache | Loco の `AppContext` component を先に使う | 原則不要 |
| API key/RLS/tenant session | Loco auth だけでは要件不足、既存 sqlx は妥当 | 追加不要 |
| sqlite-vec | sqlite-vec + direct `libsqlite3-sys` が必要 | 追加不要 |
| MCP Streamable HTTP | 既存 rmcp + Loco `after_routes` | 追加不要 |
| request/model testing | Loco testing + existing serial/axum-test/tempfile 等 | 追加不要 |
| distributed rate limit / shared cache | 要件が発生した時に既存 Loco cache と queue backend を比較 | まだ不要 |

監査範囲では、Loco/SeaORM の標準機能を置き換える目的だけの新規 crate は見当たらない。`sqlx`、`sqlite-vec`、`libsqlite3-sys`、`rmcp`、embedding stack は Loco が提供しない具体的な capability のためである。
