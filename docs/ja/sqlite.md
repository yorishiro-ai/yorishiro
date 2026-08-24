# SQLite

[English](../sqlite.md) | **日本語**

Yorishiroのマイグレーション(`migration/`)は、PostgreSQLだけでなくSQLite上でも正しいスキーマを生成する。
本ドキュメントは、現時点で何がカバーされていて何がカバーされていないかを説明する。

## 現状: スキーマのみ

本稿執筆時点で、SQLite対応として存在するのはマイグレーションセットのみである。
`src/db.rs`と`Hooks::after_context`(`src/app.rs`)は、いまもPostgreSQL専用の接続(生の`sqlx::PgPool`と、それをラップするSeaORMの`DatabaseConnection`)しか構築しないため、アプリケーション本体はまだSQLiteファイルに対して動作しない。
マイグレーションクレート自身のテストスイートが行っているように、SQLiteのURLに対して`Migrator::up`を実行すると正しく完全なスキーマができるが、アプリケーションを実際にSQLiteファイルへ接続させる配線は別の、後続の作業である。

## SQLiteが想定する用途

SQLiteは単一テナントに限定される。
PostgreSQLの行レベルセキュリティのようなデータベース側で強制されるテナント間分離を持たないため、複数テナントのホスティングではなく、お試し利用や個人利用を想定している。
このエンジンで疑似的にマルチテナント分離を作るためのアプリケーションレベルのフィルタは、意図的に実装していない。
1つのクエリでフィルタを書き漏らせばそれがそのまま黙ったテナント分離の破れになるためで、これはまさに行レベルセキュリティがPostgreSQL上で構造的に不可能にしている種類の失敗である。

## PostgreSQL版スキーマとの違いとその理由

PostgreSQL固有の機能はSQLiteに対応物が無いため、近似で置き換えるのではなく単純に省いている。

- **ロール、GRANT、行レベルセキュリティ。** 単一テナント・単一ファイルのデータベースには分離すべき第二のテナントが存在しないため、ロールやポリシーが守るべき対象そのものが無い。
- **`authenticate_api_key`(SECURITY DEFINER関数)。** PostgreSQL上でこの関数が存在するのは、未認証の呼び出し元からはRLSが隠すはずの行を読むためだけである。SQLiteにはRLSが無いので回避すべき対象も無く、アプリケーションはこのバックエンドでは`identity_api_keys`/`identity_workspaces`を直接クエリする。
- **カラムのデフォルト値としての`uuidv7()`。** SQLiteにはこの関数が無いため、このバックエンドでは`id`カラムにデフォルト値を持たせない。すべてのINSERTはアプリケーション側で生成したidを渡しており、これはもともとデータベース側のデフォルトとは無関係に行っていたことである。

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

`migration/src/helpers.rs`に、バックエンドで条件分岐するヘルパー(`enable_rls_with_policy`、`grant`、`pg_only`、`sqlite_only`、`create_table_with_checks`、`uuidv7_pk`)がすべてまとまっており、それぞれが`manager.get_database_backend()`を確認する。
各マイグレーションファイルはバックエンドを自分で判定するのではなく、これらのヘルパーを呼び出す形をとる。
結果として生成されるPostgreSQLのスキーマ(すべてのテーブル、カラム、制約名、インデックス、ポリシー、GRANT)は、SQLite対応が入る前と変わっていない。
一部の制約を発行するSQL文自体は`create_table_with_checks`/`pg_only`経由に書き換わっており、`identity_maintenance`の3つのCHECKは、以前は1回の`execute_unprepared`呼び出しにセミコロン区切りの`ALTER TABLE`文を3つまとめていたものが、いまは同じ効果を持つ3回の別々の呼び出しになっている。

## 現在のSQLite出力に関する2つの注意点

SQLiteは既定では外部キーを強制しない。接続側が自分で`PRAGMA foreign_keys = ON`を実行する必要がある。マイグレーション後のスキーマに現れる`FOREIGN KEY`宣言はすべて存在してはいるが、接続先がこのプラグマを設定するまでは効力を持たない。

SQLite上の`CURRENT_TIMESTAMP`は、sea_queryが`timestamp_with_timezone_text`と名付けたカラムに対して、`YYYY-MM-DD HH:MM:SS`(オフセット無し)という形式でレンダリングされる。このカラムはその名前とは裏腹に、実体はただのSQLiteの`TEXT`カラムであり、PostgreSQLの`timestamptz`のようなタイムゾーン対応のストレージはこのバックエンドには存在しない。
