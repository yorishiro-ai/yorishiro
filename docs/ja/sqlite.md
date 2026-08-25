# SQLite

[English](../sqlite.md) | **日本語**

Yorishiroのマイグレーション(`migration/`)は、PostgreSQLだけでなくSQLite上でも正しいスキーマを生成する。
本ドキュメントは、現時点で何がカバーされていて何がカバーされていないかを説明する。

## 現状: スキーマ、単一テナントガード、そして`/setup` → `/whoami`

SQLiteのURLに対して`Migrator::up`を実行すると正しく完全なスキーマができ、`tenancy::create_tenant`は`YORISHIRO_MAX_TENANTS`とは無関係に単一テナントの上限を強制する。
それに加えて、アプリケーション本体もいまやSQLiteファイルに対して実際に起動し、`POST /setup`と`GET /api/whoami`を実際に処理する。
setupはそのデプロイメントの唯一のテナント・ワークスペース・ユーザー・APIキーを作成し、`/whoami`はそのキーを認証して解決されたアイデンティティを返す。
テナントを作成しうるもう1つの経路である`POST /auth/signup`もSQLite上で動作し、上限に達すると2回目の`/setup`呼び出しと同様、`409`とSQLite固有の対処メッセージで拒否される。

`Authorized<R>`、`AuditAuthorized`、`Verified<R>`のいずれかの抽出器を使う認証済みルートは、まだ移植されていない。
これらはいまも無条件に`db_handle()`を呼び出しており、このバックエンドでは`DbHandle`が一切構築されないため(後述「バックエンド分岐ロジックの置き場所」を参照)、`DbHandle missing`で失敗する。
SQLite用の分岐を持つのは`AuthContext`(スコープもトランザクションも持たない)だけである。

`config/sqlite.yaml`は手動検証用の環境(`LOCO_ENV=sqlite`)であり、どのテストスイートにも組み込まれていない。
`tests/`はいまもPostgreSQL専用のままである。
`queue:`ブロックは無く(`ForegroundBlocking`ワーカー)、セットアップウィザードが有効と判定されるには他の環境と同様に`YORISHIRO_MAX_TENANTS`が設定されている必要がある。
ウィザードが実際に走った後は、SQLiteの上限そのものはこの変数の値を無視するが、`wizard_enabled()`は`/setup`の実行を許可する前に、変数が設定されていること自体はやはり確認する。

## SQLiteが想定する用途

SQLiteは単一テナントに限定される。
PostgreSQLの行レベルセキュリティのようなデータベース側で強制されるテナント間分離を持たないため、複数テナントのホスティングではなく、お試し利用や個人利用を想定している。
このエンジンで疑似的にマルチテナント分離を作るためのアプリケーションレベルのフィルタは、意図的に実装していない。
1つのクエリでフィルタを書き漏らせばそれがそのまま黙ったテナント分離の破れになるためで、これはまさに行レベルセキュリティがPostgreSQL上で構造的に不可能にしている種類の失敗である。

## 単一テナントガード

`tenancy::create_tenant`(`src/models/tenancy.rs`)は、PostgreSQL上では`YORISHIRO_MAX_TENANTS`を`count_tenants`と突き合わせて強制しており、その前に`db::lock_for_update`を取得することでカウント後にINSERTするまでの間のTOCTOUの隙を塞いでいる。
SQLite上ではこの上限を`YORISHIRO_MAX_TENANTS`からはまったく読まない。
上限は1に固定されており、このバックエンドでは環境変数は意図的に何の効果も持たない。
`YORISHIRO_MAX_TENANTS`を上げるという行為は、設定可能なポリシーを緩めるものである。
SQLite上でこの上限が存在するのは、分離の仕組み(RLS)そのものが無いからであって、ポリシーの都合ではないため、設定で回避できてはならない。

`db::lock_for_update`(`src/db.rs`)は、SQLite上では代替ロックではなくno-opになる。
SQLiteには`pg_advisory_xact_lock`に相当するものが無いためである。
それでもこれは単なる便宜ではなく、レースに対して安全である。
SQLiteは同時に1つの書き込みトランザクションしか許さないため、あるトランザクションが古いカウントを読んだあとに、別のトランザクションがその間に書き込んでコミット済みだった場合、その後のコミットは`SQLITE_BUSY`となり、2件目のテナントがコミットされるのではなく、トランザクション全体が失敗する。
PostgreSQL上でロックが塞いでいるTOCTOUは、SQLite上では黙って矛盾した書き込みが通るのではなく、リトライ可能なエラーとして現れる形になる。
この根拠の詳細、および1トランザクション内で複数行を書き込む他のロック呼び出し箇所もこれで説明がつく理由は、`lock_for_update`のドキュメントコメントを参照。

`uuidv7()`をデフォルト値に持つすべての`id`カラム(`identity_tenants`、`identity_workspaces`、`identity_users`、`identity_tenant_memberships`、`identity_api_keys`)は、`ActiveModelBehavior::before_save`を通じてSQLite上で自分のidを生成する。
この`before_save`は`db::sqlite_generated_id(conn, self.id)`を呼び出し、`id`がすでに`Set`済みか、バックエンドがPostgreSQLの場合はno-op、それ以外は`Uuid::now_v7()`を返す。
これは通常の`ActiveModel::insert()`/`.save()`呼び出しはすべてカバーするが、`Entity::insert(active).on_conflict(...).exec(conn)`はカバーしない。
このビルダー経路は`before_save`を呼び出さないため(`sea-orm` 2.0.2のソースで確認済み)、これを使っている唯一の呼び出し箇所である`tenancy::add_member`は、フックに頼らず`id`を明示的に設定している。
今後`on_conflict`を使うINSERTを追加する場合も、同様に明示的な対応が必要であり、`before_save`だけではカバーされない。

## PostgreSQL版スキーマとの違いとその理由

PostgreSQL固有の機能はSQLiteに対応物が無いため、近似で置き換えるのではなく単純に省いている。

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

## バックエンド分岐ロジックの置き場所

`migration/src/helpers.rs`に、マイグレーション用のバックエンド条件分岐ヘルパー(`enable_rls_with_policy`、`grant`、`pg_only`、`sqlite_only`、`create_table_with_checks`、`uuidv7_pk`)がすべてまとまっており、それぞれが`manager.get_database_backend()`を確認する。
各マイグレーションファイルはバックエンドを自分で判定するのではなく、これらのヘルパーを呼び出す形をとる。
結果として生成されるPostgreSQLのスキーマ(すべてのテーブル、カラム、制約名、インデックス、ポリシー、GRANT)は、SQLite対応が入る前と変わっていない。
一部の制約を発行するSQL文自体は`create_table_with_checks`/`pg_only`経由に書き換わっており、`identity_maintenance`の3つのCHECKは、以前は1回の`execute_unprepared`呼び出しにセミコロン区切りの`ALTER TABLE`文を3つまとめていたものが、いまは同じ効果を持つ3回の別々の呼び出しになっている。

アプリケーション層では、`Hooks::after_context`(`src/app.rs`)が`DbHandle`やデフォルトの`Authenticator`を構築する前に`ctx.db.get_database_backend() != DatabaseBackend::Sqlite`を確認し、`AuthContext`の`FromRequestParts`実装(`src/controllers/extractors.rs`)も同じ条件を確認して、`services::auth::authenticate_sqlite`/`touch_last_used_sqlite`とPostgreSQL用の`Authenticator`のどちらを使うかを選ぶ。
`db::sqlite_generated_id`(`before_save`から呼ばれる。前述「単一テナントガード」を参照)も同様に`conn.get_database_backend()`を確認する。
どの分岐も設定フラグや環境変数を読んで判定するのではなく、常にその場の接続から読み取っている。

## 現在のSQLite経路に関する注意点

SQLiteは既定では外部キーを強制しない。
接続側が自分で`PRAGMA foreign_keys = ON`を実行する必要がある。
マイグレーション後のスキーマに現れる`FOREIGN KEY`宣言はすべて存在してはいるが、接続先がこのプラグマを設定するまでは効力を持たない。

SQLite上の`CURRENT_TIMESTAMP`は、sea_queryが`timestamp_with_timezone_text`と名付けたカラムに対して、`YYYY-MM-DD HH:MM:SS`(オフセット無し)という形式でレンダリングされる。
このカラムはその名前とは裏腹に、実体はただのSQLiteの`TEXT`カラムであり、PostgreSQLの`timestamptz`のようなタイムゾーン対応のストレージはこのバックエンドには存在しない。
このカラムのデフォルト値経由で書き込まれた値と、アプリケーションが書き込んだ値(`chrono::Utc::now()`、例えば`touch_last_used_sqlite`による`last_used_at`の更新)は、同じカラムの中で異なるテキスト形式になる —`2026-08-24 14:27:08`と`2026-08-24T15:37:02.437013178+00:00`。
パースすればどちらもタイムスタンプとして正しく比較できるが、文字列としては比較できない。
このコードベースには現時点でこれらのカラムを生の文字列比較で並べ替えている箇所は無いが、将来そうするクエリを書く場合はまずパースが必要になる。

`sqlx::postgres::PgPoolOptions::connect`は、`sqlite://`のURLに対してエラーを返さない —無期限にハングする(直接プローブして確認済み)。
これが、`after_context`のPostgreSQLプール構築をSQLite上では試みて早期に失敗させるのではなく、丸ごとスキップしている理由である。
このバックエンドでこのコード経路に実際に到達すると、診断可能なエラーを出す代わりに、ログ出力の無いままブート自体がハングしてしまう。
