# 貢献ガイドライン

## データベースプール

データベース接続プールは、ワーカープールやキューとは別のものです。
PostgreSQL では、`AppContext.db` は migration role を使う Loco の SeaORM プール、`TenantDb` は `ROLE yorishiro_app` を設定する RLS 用の別プール、`DbHandle.identity` は migration role を使うもう一つの raw sqlx プールです。
Loco のキュープロバイダも独立したプールを持ちます。
SQLite には `DbHandle` はなく、`AppContext.db` とキュープロバイダの別 SQLite プールを使います。

本番の `database.max_connections` の既定値は、設定で上書きしない限り両バックエンドで `100` です。
最小接続数の既定値は `1` です。
CI は PostgreSQL の `DB_MAX_CONNECTIONS` を `100` に上書きし、SQLite のテスト設定ではアプリケーションプールの最大値を `10` にしています。
これらの上限は独立したプールごとの値なので、データベース接続の合計は 1 プロセスが作る各プールの合計です。
非同期の待機中はタスクが他の処理へ譲られますが、1 本の物理接続が同時に複数の SQL を実行できるわけではありません。

## テスト

テストスイートは Rust の既定の並列実行で起動してください。

PostgreSQL の実行例:

```console
$ DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15432/yorishiro \
    DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres \
    cargo test --locked --workspace
```

SQLite の実行例:

```console
$ DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' \
    LOCO_ENV=test_sqlite cargo test --locked --workspace
```

リクエストテストは終了前に `close_app_pools` を呼び出してください。

メタスキーマのバリデーションには、任意のスキーマ形式の定義と空の `entity_types` の拒否を検証するプロパティベーステストが含まれています。

## push 前の確認

```console
$ cargo check --locked --workspace
$ cargo clippy --locked --workspace --tests -- -D warnings
$ cargo fmt --all -- --check
```
