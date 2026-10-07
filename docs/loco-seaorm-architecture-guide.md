# LocoとSeaORMの境界を保つ実装ガイド

本稿の結論は、**本リポジトリの基本構造はLoco 1.2.0とSeaORM 2.0.4の推奨形に概ね沿っており、現時点でrepository層や別ORMを追加する理由はない**ということです。通常のCRUD・検索・関連・トランザクションはSeaORMのEntity APIに置き、RLS、セッション状態、pgvector/sqlite-vec、FTS、advisory lockなどORMが表現しにくい領域だけをraw SQLまたはsqlxで扱います。`edition/`のComposition root、`src/models/_entities/`の生成物、table-owned model extension、Locoのworker/task/initializer登録を今後も維持することが、重複実装と境界漏れを防ぐ最短経路です。未使用のLoco標準機能は欠落ではなく、メール・ストレージ・キャッシュ・標準JWTを現状採用していない結果です。依存調査では削除してよい直接依存は確認されていませんが、`jsonwebtoken`、`tokenizers`などの複数バージョンは将来の更新時に要確認です。本稿は調査ノートを統合したリポジトリ内の参照文書であり、事実と提案を明確に分けています。

## 目的と使い方

この文書は、新しい機能の配置、DBアクセス方式、raw SQL、依存追加の判断をレビュー前に揃えるための設計ガイドです。Loco/SeaORMの一般論ではなく、調査日（2026-10-07）時点の本リポジトリを基準にしています。根拠となる調査ノートは、[Loco監査](../research_notes/Loco%20SeaORM%20architecture%20audit/loco.md)、[SeaORM監査](../research_notes/Loco%20SeaORM%20architecture%20audit/seaorm.md)、[依存監査](../research_notes/Loco%20SeaORM%20architecture%20audit/dependencies.md)です。

「事実」は確認済みのコード配置・依存・利用状況、「提案」は今後の実装やレビューで守る判断ルールとして記述します。行番号は調査時点の参照値なので、編集後は再確認してください。まず配置と責務を確認し、次にSeaORMまたは既存のLoco機能で実現できるかを調べ、それでも不足する場合だけ独自実装・raw SQL・依存追加を検討します。

## 結論

### 事実

`edition/app.rs:38-107`に唯一の`Hooks`実装があり、`src/app.rs`が設定、ルート、worker、task、initializerなどのコミュニティ側配線を担います。`src/lib.rs:8-12`がComposition rootを再exportし、`src/`からenterprise overlayを参照しない境界を維持しています。生成Entityは`src/models/_entities/`に隔離され、手書きのtable extensionは`src/models/<table>.rs`に置かれています（[Loco監査](../research_notes/Loco%20SeaORM%20architecture%20audit/loco.md#L19-L36)）。

通常のEntity APIは広く利用され、`find`、filter、projection、join、`update_many`、`delete_many`、upsert、transaction、paginationが確認されています（[SeaORM監査](../research_notes/Loco%20SeaORM%20architecture%20audit/seaorm.md#L80-L99)）。repository traitや汎用DAOは存在しません（同#L7-L18）。この構造をrepository層へ置き換えると、table ownershipと`DatabaseConnection`/`DatabaseTransaction`、identity/tenant poolの境界を二重化するリスクがあります。

独自の二重pool、RLS session lifecycle、SQLite extension登録、domain lifecycleは、Loco/SeaORMが提供しない具体的な要件を補うものです。`sqlx`、`libsqlite3-sys`、`sqlite-vec`、`rmcp`、embedding stackも同じ理由で追加依存の重複とは判断されていません（[依存監査](../research_notes/Loco%20SeaORM%20architecture%20audit/dependencies.md#L13-L32)）。

### 提案

新しい実装は、まずLocoのcomponent・hook・worker・task・initializer、次にtable-owned SeaORM model、最後に限定的なraw SQLという順序で設計してください。repository、汎用service、別ORM、通常CRUDのraw SQL化は採用しません。これは既存実装をすべて変更する提案ではなく、今後の追加を同じ境界に収める提案です。

## Loco設計と配置

### 事実：Locoのcompositionと責務分担

Locoの`AppContext`は`db`、`config`、`mailer`、`storage`、`cache`、`queue_provider`、`shared_store`などを持ち、`Hooks`は設定読込、boot、initializer、middleware、route、worker、task、seed、shutdownをcomposition pointとして提供します。対応する本リポジトリの入口は`edition/app.rs`と`src/app.rs`です。`AppContext.shared_store`にはresolver、dispatcher、settings、poolなどをTypeId keyed DIとして登録しています（`src/app.rs:59-72,391-415`、[Loco監査](../research_notes/Loco%20SeaORM%20architecture%20audit/loco.md#L19-L27)）。

配置の責務は明確です。`src/controllers/`はREST、MCP、extractor、middlewareなどの入口、`src/models/`はtable操作とドメイン不変条件、`src/workers/`はrequestを越えるdurable work、`src/tasks/`はCLIと運用処理、`src/initializers/`はプロセス寿命の監視、`config/*.yaml`と`src/data/config.rs`は設定です。MCPもentry pointなので`src/controllers/mcp/`に置かれ、workerやtaskと同じく`src/app.rs`から明示登録されます。

### 事実：存在するが現状未利用の標準機能

`ctx.mailer`、`ctx.storage`、`ctx.cache`はLoco標準componentですが、現状のアプリコードでは利用されていません。development configにはSMTP設定が残る一方、メール送信実装はなく、production側はopt-in構成です。標準JWT/authもproductionでは使わず、API key認証を`src/models/api_keys.rs:96-154`で実装しています。これらは未使用だから削除すべき機能ではなく、同じ責務を追加する要件が発生した場合の第一候補です（[Loco監査](../research_notes/Loco%20SeaORM%20architecture%20audit/loco.md#L38-L47)）。

development schedulerは`config/development.yaml:47-53`で有効ですが、productionの`config/production.yaml:64-70`ではコメントアウトされています。Enterpriseのreindexをproductionで必要とする要件があるなら、worker常駐loopへの置換ではなく、Loco schedulerからTaskを起動し、Taskからqueue workerへ渡す構造を維持したまま設定を確認します。これはコード欠陥と断定できず、運用要件の確認事項です。

### 提案：新機能の配置

入口は認証・入力解析・モデル呼出し・renderに限定し、普通のfinder、query、state transitionはtable extensionへ置きます。requestを越える処理は`BackgroundWorker`、手動または一回の運用処理は`Task`、定期処理はschedulerのTask entryにします。`tokio::spawn`はinitializerのプロセス寿命監視や明示的な並行処理に限り、request処理の代替にしません。

メール、ファイル、共有cacheが必要になったときは、まず`ctx.mailer`、`ctx.storage`、`ctx.cache`のdriverと設定を確認します。直接SMTP、S3、cache singletonを追加するのは、標準componentが要件を満たせないと記録できた場合だけです。設定可能な値は`config/*.yaml`とtyped `Settings`/`ctx.config`に置き、`src/`で新たな`std::env::var`を増やしません。

## SeaORM設計と配置

### 事実：Entity、ActiveModel、migration

`src/models/_entities/`は`cargo loco db entities`などで生成されるファイルで、手編集対象ではありません。application-owned extensionは同じtable名の`src/models/<table>.rs`に置き、ActiveModel hook、finder、table-owned persistenceをそこで実装します。大きなmodelは`src/models/entity_entities/`のようなprivate moduleに分割できます（`src/models/mod.rs:1-45`、[SeaORM監査](../research_notes/Loco%20SeaORM%20architecture%20audit/seaorm.md#L61-L79)）。

通常のCRUD、filter、sort、join、projection、bulk mutation、upsertはEntity APIまたはSeaQuery builderを優先します。`select_only`、`column_as`、`into_model`、`into_tuple`、`update_many().col_expr()`、`OnConflict`、`TransactionTrait`が実際に利用されています。table modelから`EntityRecord`や`WorkspaceRecord`などへ変換し、DB Entityをwire DTOへ直接漏らさない方針も確認できます（同#L80-L99）。

migrationは`migration/src/mYYYYMMDD_*.rs`に置き、`migration/src/lib.rs`へ登録します。Table、Index、ForeignKey、alterはSeaQuery builderを標準とし、RLS、GRANT、role、trigger、custom type、GIN/HNSW、backend固有型だけraw DDL helperを使います。`migration/src/helpers.rs:23-70,115-230`はUUID、JSON、timestamp、check、RLS、GRANTをbackend差分として集約しています。

### 事実：pool境界とtransaction

`TenantDb::begin_for_workspace`はSeaORMの`DatabaseTransaction`を返し、そのtransaction上でEntity APIとraw SQLを同じRLS scopeに載せます（`src/db.rs:163-192`）。signup/provisioningではtransactionを引数に渡して複数tableのcommit ownershipを明示します。SQLiteはRLS/roleを持たずsingle-tenant制約があるため、PostgreSQLのtenant pool abstractionを無理に同一化していません（[SeaORM監査](../research_notes/Loco%20SeaORM%20architecture%20audit/seaorm.md#L175-L187)）。

### 提案：SeaORMとraw SQLの選択

通常のtable CRUDはEntity APIにします。raw SQLを選べるのは、RLS/session GUC、advisory lock、pgvector/sqlite-vec、FTS5/pg_trgm、JSONB containment、single-statement atomicity、backend固有DDLなど、SeaORMに同等の表現がないか、一文の競合制御が意味を持つ場合です。値はbind parameter、動的identifierは固定値またはallowlist由来に限定し、PostgreSQLの`$N`とSQLiteの`?`をbackend間で共有しません。

新しいraw queryには、同じmoduleに理由、対象backend、pool/transaction、bind方針、identifierの安全性、回帰テストを残します。SeaQueryで表現できる普通のfilterやjoinをraw SQLへ移す場合は、可読性・性能・backend parityの測定結果が必要です。

## 既存実装の監査

### Loco標準からの意図的な拡張

二重poolとRLS session lifecycle（`src/db.rs:1-11,52-64,123-180`）は、接続取得・返却時の`SET ROLE`、`RESET`、GUC、advisory lockを扱うための境界です。Loco標準DB接続の単純な置換やrepository化では代替できません。Loco queue上の`queue_job_lifecycles`と`workers/lifecycle.rs`も、retryそのものではなくadmission、heartbeat、reconcile、domain stateを記録する業務層です。Loco queueと責任範囲を文書化すれば維持できます。

`src/controllers/middleware/rate_limit.rs:19-24`のprocess-local rate limitは、共有cacheの代替ではありません。単一processの明示的な制限に留める限り追加抽象化は不要です。複数replicaで共有する要件が出たときだけLoco cache、queue backend、DB lockを比較します。`src/services/`はembeddingなど外部clientに限定され、domain serviceやrepositoryを置いていない点も現行ルールに適合します。

### raw SQLの妥当性と監視点

`src/models/api_keys.rs`のSECURITY DEFINER認証とbest-effort timestamp、`src/models/search.rs`のvector/FTS/trigram検索、`src/db.rs`のsession state/advisory lock、migrationのRLS/GRANT/trigger DDLは、ORMだけでは権限またはbackend固有意味を保ちにくい領域です。`src/models/search.rs:305-311`のembedding table allowlist、各backendで分けたplaceholder、bind値は維持すべき防御です。

一方、`ee/models/schema_schemas/query.rs:42-69`はPostgreSQL固定queryなので、SQLiteで到達可能なら明示guardまたはテストを追加します。migrationの「repository layer」というコメント（`migration/src/m20260829_000000_initial_schema.rs:572,1439`）は現行のtable-owned model/pool設計と用語がずれるため、将来修正する候補です。これは実装変更を伴わない文言改善です。

### 依存追加・削除の監査

`cargo metadata --locked`、`cargo machete 0.9.2`、ソース横断検索の範囲では、削除してよい直接依存は確認されていません。`sqlx`はpool lifecycle、RLS、GUC、advisory lock、system accessに、`libsqlite3-sys`はsqlite-vecのauto-extension登録に、`rmcp`/`schemars`はMCP protocol/schemaに、`jsonschema`はvalidationに、Candle/tokenizersはlocal embeddingに使われています（[依存監査](../research_notes/Loco%20SeaORM%20architecture%20audit/dependencies.md#L1-L32)）。

依存の重複として`jsonwebtoken` 10.x/11.x、`tokenizers` 0.22.x/0.23.x、`serde_yaml`/`serde_yaml_ng`、暗号系crateの複数versionが確認されています。ただしupstreamやfeatureの要求差があるため、重複だけで削除・統合を断定しません。`cargo tree -i <crate>`とfeature別build、該当integration testを先に実行します。

## 依存追加/削除の判断表

| 要求 | まず使う既存機能 | 判断 | 確認事項 |
|---|---|---|---|
| 通常のDB CRUD・migration | SeaORM Entity API、SeaQuery、sea-orm-migration | 追加不要 | model ownershipとmigration登録 |
| queue、retry、scheduler、task | Loco worker/task/scheduler | 追加不要 | route/task/worker登録とconfig |
| mail、storage、cache | `AppContext`の標準component | 原則追加不要 | driver、設定、未使用理由 |
| API key、RLS、tenant session | 既存`sqlx`と`src/db.rs` | 維持。別ORM不要 | pool、role、GUC、transaction |
| vector、FTS、trigram | 既存raw SQL、sqlite-vec、pgvector | 維持 | backend分岐、allowlist、回帰テスト |
| MCP | `rmcp`と`src/controllers/mcp/` | 維持 | `after_routes` mount |
| local embedding | Candle、tokenizers、既存services | 維持 | optional化は要件変更時のみ |
| distributed rate limit/shared cache | Loco cache、queue backend、DB lockを比較 | まだ追加しない | replica要件と性能測定 |
| 新しいORM/query builder | SeaORM/SeaQueryで表現不能か検証 | 原則不採用 | capability gapを記録 |
| 既存依存の削除・version統合 | `cargo tree`、feature build、tests | 要確認 | API/feature/license互換性 |

## 今後の実装チェックリスト

### 事実に基づく確認項目

1. 新しいtableならmigrationを`migration/src/`に追加し、`migration/src/lib.rs`へ登録する。
2. `make entities`で生成Entityを更新し、`src/models/_entities/`を手編集しない。
3. 必要なdomain操作を`src/models/<table>.rs`またはtable-owned private moduleへ追加する。
4. controller、MCP、task、workerはモデルAPIを呼び、通常queryを自分で組み立てない。
5. requestを越える仕事はworker、運用処理はtask、定期実行はscheduler経由にする。
6. raw SQLではbackend、pool、transaction、bind、dynamic identifier、理由を確認する。
7. `cargo loco routes`、`cargo loco scheduler --list`、対象testを実行する。
8. CE変更は`make check-all`、EEまたは共有API変更は`make check-all-ee`を実行する。

### 提案として追加するレビュー質問

- 既存のLoco componentまたはSeaORM APIで代替できないことをコード上の根拠で説明できるか。
- transactionの開始、commit、rollbackの所有者とidentity/tenant poolが型・引数から明らかか。
- SQLiteとPostgreSQLでID、JSON、timestamp、placeholder、RLSの差を意識しているか。
- 新しいbulk insert、eager loading、streamingがActiveModel hook、RLS、memory、connection lifetimeを壊さないか。
- 依存追加なら、既存crateの不足機能、feature条件、license、integration testまで確認したか。

## 優先度付きアクション

**P0（新規実装の必須境界）**として、repository層・汎用DAO・別ORMを追加せず、通常DB操作をtable-owned SeaORM modelに置きます。raw SQLは既存の例外領域だけに限定し、bindとidentifier allowlistを必須レビュー項目にします。これは既存設計の中心を守るアクションです。

**P1（安全性と運用の固定）**として、Enterprise PostgreSQL-only queryに到達不能契約またはbackend guardを追加し、raw queryごとのintegration testを維持します。productionでreindexが必要か、scheduler設定が意図的に無効なのかも運用担当と確認します。

**P2（保守性）**として、migrationの古い「repository layer」コメントを現行用語へ直し、未使用SMTP設定をdevelopmentでどう扱うか決めます。`cargo tree -i`で複数versionを定期確認しますが、互換性検証なしの統合は行いません。

**P3（要件発生時のみ）**として、メール、storage、cache、shared rate limitを導入する際にLoco標準componentを先に評価します。標準機能で不足する場合だけ、不足するAPI、性能、backend、運用要件を記録して追加依存を選定します。

## 調査限界

本稿は指定された3つの調査ノートと、そのノートが参照したコード位置に基づく監査です。調査ノートではfull `make check-all`、PostgreSQL/SQLite integration suiteは未実行とされています。したがって、依存削除の安全性、SQLiteでの全到達経路、production schedulerの運用意図、将来の性能特性は要確認です。

また、`cargo machete`や`rg`はmacro展開、feature条件、generated codeの意味を完全には判定しません。Cargo.lockのduplicateは、Loco、SeaORM、RMCP、Candleなど異なるupstream要求の結果であり、すべてをアプリ側の無駄とはみなせません。行番号は調査時点のものです。実装前には対象ファイルを再読し、変更後に該当featureのbuild、migration、PostgreSQL/SQLiteテストを実行してください。
