# SQLite

[English](../sqlite.md) | **日本語**

Yorishiroのマイグレーション(`migration/`)は、PostgreSQLとSQLiteの両方で正しいスキーマを生成する。
本ドキュメントは、現時点で何がカバーされていて何がカバーされていないかを説明する。

## 現状: スキーマ、単一テナントガード、そして大半の認証済みルート

SQLiteのURLに対して`Migrator::up`を実行すると正しく完全なスキーマができ、`tenancy::create_tenant`は`YORISHIRO_MAX_TENANTS`とは無関係に単一テナントの上限を強制する。
それに加えて、アプリケーション本体はSQLiteファイルに対して実際に起動し、`POST /setup`、`GET /api/whoami`、そしてエンティティCRUDを含む`Authorized<R>`/`AuditAuthorized`ルートをすべて処理する(残っている狭い境界の詳細は後述「まだブロックされているもの」を参照)。
setupはそのデプロイメントの唯一のテナント・ワークスペース・ユーザー・APIキーを作成し、`/whoami`はそのキーを認証して解決されたアイデンティティを返す。
テナントを作成しうるもう1つの経路である`POST /auth/signup`は、上限に達すると2回目の`/setup`呼び出しと同様、`409`とSQLite固有の対処メッセージで拒否される。

`AuthContext`、`Authorized<R>`、`AuditAuthorized`はいずれもSQLite用の分岐を持つ(`src/controllers/extractors.rs`)。
認証したうえで、後の2つは`DbHandle`/`TenantDb::begin_for_workspace`を経由せず`ctx.db`に対して直接プレーンなトランザクションを開く。
RLS前提の2プール構成は、RLSを持たない単一テナントバックエンドにはそもそもスコープすべき対象が無いためである。
`Verified<R>`だけは意図的にSQLite用の分岐を持たない。
唯一の呼び出し元(`search_entities`)がどのみち`db_handle()`を直接呼ぶうえ、このルート自体がベクトル類似検索のために`content_entities.embedding`に依存しておりこのバックエンドには存在しないため、SQLite上ではそもそも到達不能である(詳細は「まだブロックされているもの」を参照)。

`config/development.yaml`もデータベースとキューの両方をSQLiteに既定している。
したがってクローンしただけで何も設定しなくてもこのバックエンドで起動し、`LOCO_ENV`を設定しなくても初回セットアップウィザードが動作する。
`DATABASE_URL`にPostgreSQLのURIを設定すればそちらが優先される。
RLS・複数テナント・ベクトル検索のいずれかを必要とするデプロイはそうする。

`config/sqlite.yaml`は引き続き独立した手動検証用の環境(`LOCO_ENV=sqlite`)であり、どのテストスイートにも組み込まれていない。
`tests/`はいまもPostgreSQL専用のままである。
`development.yaml`・`production.yaml`と同じく`queue: kind: Sqlite`と`workers.mode: BackgroundQueue`を設定している。loco-rsのSQLiteキュープロバイダ(`bgworker::sqlt`)は`ctx.db`とは独立した`sqlx::SqlitePool`を自前で張るが、ロック競合下も含め実ファイルに対して実測で動作を確認済みである(計測内容は`docs/ja/configuration.md`の「キューのバックエンドと調整」を参照)。
セットアップウィザードが有効と判定されるには`YORISHIRO_MAX_TENANTS`が上限値として解決される必要があり、それを自分で渡すかどうかは起動するエントリポイントによって決まる。
ベース版のバイナリ(`src/bin/main.rs`)は、運用者が設定していなければ`1`を設定する。
したがってSQLite層を何も設定せずに起動しても`POST /setup`は動作し、こちらが通常のケースである。
`ee/`のバイナリは何も設定しないため、有償版のデプロイではウィザードが応答する前にこの変数を明示的に設定する必要がある。
テストハーネスもどちらのバイナリの`main`も経由せずに`App`を起動するため、同じ挙動になる。
ウィザードが実際に走った後は、SQLiteの上限そのものはこの変数の値を無視するが、`wizard_enabled()`は`/setup`の実行を許可する前に、上限値として解決されること自体はやはり確認する。

## タグを指定せずに起動したワーカーは何も処理しない

キュープロバイダが動作することと、ジョブが実際に実行されることは別である。
この実装がキューに入れるジョブはすべて`worker-class:*`のタグをちょうど1つ持つ一方、`--worker`(あるいは`--server-and-worker`や`-a`)で起動したワーカーはタグなしのジョブだけを購読するため、これらのジョブを1件も取り出さない。

しかも、この状態はどこにも通知されない。
書き込みは成功し、ジョブ行はタグ付きで`sqlt_loco_queue`に入り、ワーカーは起動してポーリング中だとログに出す。
それでいてジョブは`queued`のまま永久に残り、どちら側にもエラーは出ない。
実機のSQLite環境で計測した結果は次のとおりである。
`EmbeddingSyncWorkerShared`のジョブ3件が`--server-and-worker`のプロセスでも素の`--worker`のプロセスでも`queued`のまま動かず、タグを明示した途端にただちに処理された。

そのプロセスに担当させたいクラスは、すべて明示的に並べる。

```sh
yorishiro_core-cli start --worker=worker-class:tenant-private,worker-class:official,worker-class:shared
```

どれを購読したかはワーカー自身の起動行でわかる。
単なる`worker is online`ではなく`worker is online with tags: ...`と出る。
ワイルドカードは存在しない。
またこれはSQLite固有の話ではなくPostgreSQLのキューでもまったく同じであり、理由・複数プロセス構成・デプロイが購読し続けるべき範囲は`docs/ja/configuration.md`の「サーバとは別プロセス・別ホストでワーカーを動かす」が扱っている。

## 埋め込みプロバイダ未設定時、ジョブは失敗するがエンティティの書き込みは成功する

`YORISHIRO_EMBEDDING_BASE_URL`・`YORISHIRO_EMBEDDING_MODEL`が未設定のデプロイでも、起動もエンティティのCRUDも問題なく動く。
表面化するのは`x-embed`を持つフィールドを含むスキーマに対して最初のエンティティを書き、そのジョブの`worker-class:*`タグを購読しているワーカーが実際にそれを取り出した時点である。
そのようなワーカーがいなければ、ジョブはプロバイダまで到達しない。
前節のとおり`queued`のまま残り、以下に書くことは何も起こらない。
ワーカーがいる場合は、ジョブが`UnconfiguredEmbeddingProvider`に到達して失敗し、`sqlt_loco_queue`上で`failed`となり、設定すべき2つの変数がログに残る。

```text
WARN embedding sync failed transiently, job will be marked failed for retry_failed
  error=embedding provider unreachable at : no embedding provider is configured:
        set YORISHIRO_EMBEDDING_BASE_URL and YORISHIRO_EMBEDDING_MODEL
```

一方でエンティティの書き込み自体は`201`を返し、行はコミットされている。
埋め込みはあくまで補助的な機能であり、直前の書き込みを妨げることは決してない。
したがってジョブの失敗が意味するのは、そのエンティティにベクトルが付いていないことだけであって、何かが失われたわけではない。
なお`x-embed`を持つフィールドが1つもないスキーマでは、そもそもプロバイダまで到達しない。
埋め込む対象のテキストが存在しないため、プロバイダの設定有無にかかわらずジョブは何もせずに完了する。

## `database.max_connections`はSQLite上で2以上が必須

`Authorized<R>`/`AuditAuthorized`は、リクエストの間ずっとトランザクション上で1本の接続を保持しつつ、`identity_api_keys.last_used_at`の更新は同じプールの別の独立した接続で行う。
別接続にしている理由はPostgreSQL版の`authorize`/`touch_last_used_on`と同じで、読み取り専用ハンドラはトランザクションをコミットせずに落とすため、そこで更新すると黙ってロールバックされてしまうからである。
`max_connections: 1`だと、この2本目の接続取得には空いている接続が無く、`connect_timeout`を使い切ったうえで失敗するしかない。

SQLite上で`max_connections`が2未満のまま起動しようとすると、起動そのものを拒否する(`db::require_min_sqlite_connections`、`Hooks::after_context`から呼ばれる)。負荷がかかったときに不定な壊れ方をさせるより、その場で止めたほうがいい。
このガードが入る前に実測した内容: `max_connections: 1`、`connect_timeout: 500`の状態で、読み取り専用ルート(`GET /api/relations`)は`200`のまま返った。失敗した`last_used_at`更新はbest-effortでログ警告のみだからである。
一方、本物の書き込みのために2本目の接続を自身で必要とするルート(`PUT /api/system/maintenance`。保持中のトランザクションとは独立に`ctx.db`へ書き込む)は、約500ms後に`500`で失敗し、ログには`Failed to acquire connection from pool: Connection pool timed out`と記録された。
`config/sqlite.yaml`は`max_connections: 10`で出荷されており、下限を十分に上回っている。

## SQLiteが想定する用途

SQLiteは単一テナントに限定される。
PostgreSQLの行レベルセキュリティのようなデータベース側で強制されるテナント間分離を持たないので、想定用途はお試し利用や個人利用にとどめてあり、複数テナントのホスティングは対象外である。
このエンジンで疑似的にマルチテナント分離を作るためのアプリケーションレベルのフィルタは、意図的に実装していない。
1つのクエリでフィルタを書き漏らせばそれがそのまま黙ったテナント分離の破れになるためで、これはまさに行レベルセキュリティがPostgreSQL上で構造的に不可能にしている種類の失敗である。

## 単一テナントガード

`tenancy::create_tenant`(`src/models/tenancy.rs`)は、PostgreSQL上では`YORISHIRO_MAX_TENANTS`を`count_tenants`と突き合わせて強制する。その前に`db::lock_for_update`を取得し、カウントしてからINSERTするまでの間に生じるTOCTOUの隙を塞いでいる。
SQLite上ではこの上限を`YORISHIRO_MAX_TENANTS`からはまったく読まない。
上限は1に固定されており、このバックエンドでは環境変数は意図的に何の効果も持たない。
`YORISHIRO_MAX_TENANTS`を上げるという行為は、設定可能なポリシーを緩めるものである。
SQLite上でこの上限が存在するのは、分離の仕組み(RLS)そのものが無いからであって、ポリシーの都合ではないため、設定で回避できてはならない。

`db::lock_for_update`(`src/db.rs`)は、SQLiteには`pg_advisory_xact_lock`に相当するものが無いため、このバックエンドでは代替ロックを用意せずno-opにしてある。
それで安全性が損なわれるわけではない。
SQLiteは同時に1つの書き込みトランザクションしか許さないので、あるトランザクションが古いカウントを読んだあとに別のトランザクションが先に書き込んでコミット済みだった場合、その後のコミットは`SQLITE_BUSY`で弾かれる。
2件目のテナントが実際にコミットされてしまうことはなく、トランザクション全体がそのまま失敗する。
つまり PostgreSQL 上でロックが塞いでいる TOCTOU は、SQLite 上ではリトライ可能なエラーという形で表面化する。矛盾した書き込みが黙って通ってしまうことはない。
この根拠の詳細、および1トランザクション内で複数行を書き込む他のロック呼び出し箇所もこれで説明がつく理由は、`lock_for_update`のドキュメントコメントを参照。

`uuidv7()`をデフォルト値に持つすべての`id`カラム(`identity_tenants`、`identity_workspaces`、`identity_users`、`identity_tenant_memberships`、`identity_api_keys`)は、`ActiveModelBehavior::before_save`を通じてSQLite上で自分のidを生成する。
この`before_save`は`db::sqlite_generated_id(conn, self.id)`を呼び出し、`id`がすでに`Set`済みか、バックエンドがPostgreSQLの場合はno-op、それ以外は`Uuid::now_v7()`を返す。
これは通常の`ActiveModel::insert()`/`.save()`呼び出しはすべてカバーするが、`Entity::insert(active).on_conflict(...).exec(conn)`はカバーしない。
このビルダー経路は`before_save`を呼び出さないため(`sea-orm` 2.0.2のソースで確認済み)、これを使っている唯一の呼び出し箇所である`tenancy::add_member`は、フックに頼らず`id`を明示的に設定している。
今後`on_conflict`を使うINSERTを追加する場合も、同様に明示的な対応が必要であり、`before_save`だけではカバーされない。

## PostgreSQL版スキーマとの違いとその理由

PostgreSQL固有の機能はSQLiteに対応物が無い。近似で置き換えようとはせず、単純に省いてある。

- **ロール、GRANT、行レベルセキュリティ。** 単一テナント・単一ファイルのデータベースには分離すべき第二のテナントが存在しないため、ロールやポリシーが守るべき対象そのものが無い。
- **`authenticate_api_key`(SECURITY DEFINER関数)。** PostgreSQL上でこの関数が存在するのは、未認証の呼び出し元からはRLSが隠すはずの行を読むためだけである。SQLiteにはRLSが無いので回避すべき対象も無く、アプリケーションはこのバックエンドでは`identity_api_keys`/`identity_workspaces`を直接クエリする。
- **カラムのデフォルト値としての`uuidv7()`。** SQLiteにはこの関数が無いため、このバックエンドでは`id`カラムにデフォルト値を持たせない。すべてのINSERTはアプリケーション側でidを渡す必要がある。`uuidv7_pk`でキーが振られた5つのエンティティが`before_save`経由でこれをどう扱っているかは、前述「単一テナントガード」を参照。

SQLiteでも表現はできるが構文が異なるものは、バックエンドごとに同じ保証を2通りの書き方で実装している。

- **テーブルレベルのCHECK制約。** PostgreSQL版のマイグレーションは、テーブル作成後に`ALTER TABLE ... ADD CONSTRAINT`で追加する。SQLiteの`ALTER TABLE`はリネーム・カラム追加・カラム削除しかサポートしないため、このバックエンドでは同じCHECKを`CREATE TABLE`文の中にインラインで書く。
- **削除されたテンプレートからスキーマの紐付けを外すトリガー。** PostgreSQL版は`plpgsql`関数と、それを呼び出す`CREATE TRIGGER`という2段構えで表現する。SQLiteには関数とトリガーの分離が無いため、同じ`UPDATE`文を`CREATE TRIGGER ... BEGIN ... END`のボディに直接書く。
- **`identity_templates.tags`。** PostgreSQLの`TEXT[]`配列カラムにはSQLite側の対応物が無いため、SQLite版のカラムは同じタグ一覧をJSONエンコードした`TEXT`カラムとして持つ。このカラムを読み書きするアプリケーションコードは、どちらの表現を相手にしているかを意識する必要がある。
- **`content_entity_column_preferences.columns`の配列形チェック。** PostgreSQL版は`jsonb_typeof(columns) = 'array'`と書く。SQLiteのJSON1拡張では同じチェックを`json_type(columns) = 'array'`と書く。

## まだ移植されていないもの: 埋め込みと全文検索

`content_entities.embedding`(pgvectorのカラム)と、それに紐づくインデックス(HNSWによる類似検索、GINによるJSONBインデックス、トライグラムインデックス)はSQLiteには存在しない。
このバックエンドでは、ベクトル類似検索と全文検索はまだ動作しない。
これらの移植は、pgvectorの代わりに`sqlite-vec`の`vec0`仮想テーブル(ベクトル検索用)、`pg_trgm`の代わりにSQLiteの`FTS5`拡張(全文検索用)を使う形になる見込みだが、いずれもまだ実装されていない。

## まだブロックされているもの: ベクトル検索と、`neighbors_batch`自身のPostgres専用SQL

`src/models/_entities/content_entities.rs`(`cargo loco db entities`が生成し、手で編集することは無いファイル)は、`Model`構造体上に`embedding: Option<PgVector>`を無条件に宣言しているが、このカラムはPostgreSQLにしか存在せず、SQLite版のテーブルには`embedding`カラム自体が無い。
以前はこれが原因で、このテーブルに対するクエリはすべて失敗していた。
SeaORMのEntity APIがバックエンドを問わず`Model`のすべてのフィールドからクエリを組み立てていたためである。
`count`・`get`・`get_batch`・`list`・`export_all`・`create`・`update`・`delete`(`src/models/content_entities.rs`)は、いまは内部で分岐する。
SQLite上では`embedding`を除いた列リストでクエリを組み立てて結果をデコードし(`select_record_columns`)、`create`/`update`はさらにもう1つ、別の失敗を回避する。
`ActiveModelTrait::insert`/`update`が戻り値をデコードする際にも`embedding`に触れてしまい、SeaORMの`pgvector::Vector`のデコード実装は、カラムの有無にかかわらずSQLiteの行に対して無条件にエラーを返すためである。
どちらの分岐もPostgreSQL側は一切変えておらず、この8関数を呼ぶ側(`content_relations::create`、`controllers/workspaces.rs`の`entity_count`、`recall.rs`、エンティティCRUD/エクスポート/インポートの各ルート)は、この変更による影響を受けない。
行が実際に存在するようになった今、入力・戻り値の型・エラーはどちらのバックエンドでも変わらない。

まだブロックされているもの:

- **ベクトル類似検索**(`GET /api/search`、`Verified<ReadScope>`): `content_entities.embedding`そのものを読むため、SQLiteにはまだ存在しない。前述の`Verified<R>`にSQLite用の分岐が無い理由は、実質的にこれである。
- **近傍探索**(`content_relations::neighbors_batch`。「Xに関連するエンティティ」をまとめて引く処理): `embedding`とは無関係な理由でブロックされている。この生SQLは`embedding`列を一切SELECTしていないが、`Statement::from_sql_and_values(DatabaseBackend::Postgres, ...)`をハードコードしたうえ、PostgreSQL専用の配列関数`unnest($2::uuid[])`を使っている。`embedding`のギャップが埋まる前から、SQLiteでは動く見込みが無かった。
- **`content_entity_snapshots::snapshot`**(上書きされる前のエンティティのデータを記録する`INSERT ... SELECT`。`ee/`の`infer_fill`が、モデルの推測を`content_entities`へ直接書き込む前に呼ぶ): `neighbors_batch`と同様、生のPostgres専用SQLであり、本セクションの修正の影響を受けない。

`POST /api/migration-jobs/{id}/undo`自体(`content_entities::undo_job`)には、もうSQLite用の例外は不要である。以前は`content_entities::update`を経由せず`ActiveModel::update(conn)`を直接呼んでおり、`create`/`update`がかつて直面していたのと同じ戻り値のデコード失敗に当たっていたが、いまは`update_and_fetch`と同じ修正である`active.update_without_returning(conn)`を使っている。スナップショットが存在すればSQLite上でも復元は動作するが、`snapshot`(前述)は`ee/`専用かつPostgres専用のままなので、base自体はスナップショットを一切書き込まない。

ブロックされていないもの: エンティティCRUD(`POST`/`GET`/`PUT`/`DELETE /api/entities`)、エクスポート/インポート(`GET /api/export.jsonl`、`POST /api/import.jsonl`)、`GET /api/workspaces/{id}`の`entity_count`フィールド、`POST /api/migration-jobs/{id}/undo`、リレーションの作成(`POST /api/relations`。両端のidについて`content_entities::get`を呼ぶ)。
`content_entities`に触れないため同じく影響を受けないもの: `GET`/`DELETE /api/relations/{id}`、`PUT /api/relations/{id}/status`、`GET /api/relations`(一覧)、スキーマ系(`GET`/`POST /api/schemas`、`GET /api/schemas/active/{name}`、`GET /api/schemas/{schema_id}`、テンプレート系)、`GET /api/audit-log`、`GET`/`PUT /api/system/maintenance`。
`POST /api/schemas`を`template_id`付きボディで呼ぶ場合は、`content_entities`には触れないものの知っておく価値のある部分的な例外である。
`identity_templates::resolve_template_definition`を`ctx.db`に対して呼び出しており、これはリクエスト自身のトランザクションが開いたままの状態で取得する2本目の接続である。
上記の`max_connections`下限の範囲内であれば安全で、`set_maintenance`と同じ話であり、別立ての制約ではない。

## バックエンド分岐ロジックの置き場所

`migration/src/helpers.rs`に、マイグレーション用のバックエンド条件分岐ヘルパー(`enable_rls_with_policy`、`grant`、`pg_only`、`sqlite_only`、`create_table_with_checks`、`uuidv7_pk`)がすべてまとまっており、それぞれが`manager.get_database_backend()`を確認する。
各マイグレーションファイルの側は、バックエンドを自分で判定せず、これらのヘルパーを呼び出すだけにしてある。
結果として生成されるPostgreSQLのスキーマ(すべてのテーブル、カラム、制約名、インデックス、ポリシー、GRANT)は、SQLite対応が入る前と変わっていない。
一部の制約を発行するSQL文自体は`create_table_with_checks`/`pg_only`経由に書き換わっており、`identity_maintenance`の3つのCHECKは、以前は1回の`execute_unprepared`呼び出しにセミコロン区切りの`ALTER TABLE`文を3つまとめていたものが、いまは同じ効果を持つ3回の別々の呼び出しになっている。

アプリケーション層では、`Hooks::after_context`(`src/app.rs`)が`DbHandle`やデフォルトの`Authenticator`を構築する前に`ctx.db.get_database_backend() != DatabaseBackend::Sqlite`を確認し、`AuthContext`/`Authorized<R>`/`AuditAuthorized`の`FromRequestParts`実装(`src/controllers/extractors.rs`)も同じ条件を確認して、`services::auth::authorize`/`services::auth::authenticate`の`..._sqlite`系関数とPostgreSQL用の`Authenticator`/`DbHandle`のどちらを使うかを選ぶ。
`db::sqlite_generated_id`(`before_save`から呼ばれる。前述「単一テナントガード」を参照)も同様に`conn.get_database_backend()`を確認する。
唯一の例外が`db::require_min_sqlite_connections`(前述「`database.max_connections`はSQLite上で2以上が必須」を参照)で、これだけは設定値そのものを無条件に確認する。接続がまだ存在しない起動の時点で拒否を判断しなければならないので、その場の接続を見て判断するという他の分岐と同じやり方が使えない。
それ以外の分岐はすべて、設定フラグや環境変数を読むのではなく、常にその場の接続から読み取っている。

## 現在のSQLite経路に関する注意点

SQLiteは既定では外部キーを強制しない。
接続側が自分で`PRAGMA foreign_keys = ON`を実行する必要がある。
マイグレーション後のスキーマに現れる`FOREIGN KEY`宣言はすべて存在してはいるが、接続先がこのプラグマを設定するまでは効力を持たない。

SQLite上の`CURRENT_TIMESTAMP`は、sea_queryが`timestamp_with_timezone_text`と名付けたカラムに対して、`YYYY-MM-DD HH:MM:SS`(オフセット無し)という形式でレンダリングされる。
このカラムはその名前とは裏腹に、実体はただのSQLiteの`TEXT`カラムであり、PostgreSQLの`timestamptz`のようなタイムゾーン対応のストレージはこのバックエンドには存在しない。
このカラムのデフォルト値経由で書き込まれた値と、アプリケーションが書き込んだ値(`chrono::Utc::now()`、例えば`touch_last_used_sqlite`による`last_used_at`の更新)は、同じカラムの中で異なるテキスト形式になる。前者は`2026-08-24 14:27:08`、後者は`2026-08-24T15:37:02.437013178+00:00`という形である。
パースすればどちらもタイムスタンプとして正しく比較できるが、文字列としては比較できない。
このコードベースには現時点でこれらのカラムを生の文字列比較で並べ替えている箇所は無いが、将来そうするクエリを書く場合はまずパースが必要になる。

`sqlx::postgres::PgPoolOptions::connect`は、`sqlite://`のURLに対してエラーを返さない。無期限にハングする(直接プローブして確認済み)。
だからこそ`after_context`は、SQLite上ではPostgreSQLプールの構築自体を丸ごとスキップする。試みて早期に失敗させる、という選択肢は取れない。
このバックエンドでこのコード経路に実際に到達すると、診断可能なエラーを出す代わりに、ログ出力の無いままブート自体がハングしてしまう。
