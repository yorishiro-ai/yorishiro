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

Linux のビルドはリポジトリの `.cargo/config.toml` を使うため、`mold` と `sccache` が必要です。
ローカルで Cargo を実行する前に両方をインストールしてください。

テストスイートは Rust の既定の並列実行で起動してください。
`make test-postgres` または `make test-sqlite` を使うと、共通テストを含むスイートを実行し、選択したバックエンド専用テストが 1 件以上実行されたことも検証できます。
最後に、バックエンド専用ゲートの選択数、実行数、スキップ数とスキップ理由が表示されます。
バックエンドゲートのラッパーは `uv run scripts/test_backend.py` です。
Valkey のキューと設定を専用エンドポイントで検証する場合は `make test-redis` を使います。
リポジトリのその他の補助スクリプトも uv プロジェクトから実行し、Python 標準ライブラリだけを使います。
公開された振る舞いを検証するテストは `tests/` に置きます。
テストのためだけに非公開の実装境界を公開すべきではないため、非公開の実装境界を検証するテストに限り本番コードと同じファイルに残せます。
テスト出力は、プロトコルのハンドシェイク、バックエンドゲートの報告、意図したスキップまたは失敗を説明する診断に限定します。

マイグレーションのテストでは、両方の対応バックエンドについて、初期構築、記録済みの各マイグレーション境界からの段階的なアップグレード、ロールバックを分けて検証します。
PostgreSQL のマイグレーションテストは一意な一時データベースを使い、SQLite のテストは一時ファイルを使うため、各テストが自分のデータベース資源を後始末します。

PostgreSQL の実行例:

```console
$ DATABASE_URL=postgres://yorishiro:yorishiro@localhost:15432/yorishiro \
    DB_MAX_CONNECTIONS=100 DB_CONNECT_TIMEOUT=5000 LOCO_ENV=test_postgres \
    make test-postgres
```

SQLite の実行例:

```console
$ DATABASE_URL='sqlite:///tmp/yorishiro.sqlite3?mode=rwc' \
    LOCO_ENV=test_sqlite make test-sqlite
```

リクエストテストは終了前に `close_app_pools` を呼び出してください。

メタスキーマのバリデーションには、任意のスキーマ形式の定義と空の `entity_types` の拒否を検証するプロパティベーステストが含まれています。

## push 前の確認

```console
$ cargo check --locked --workspace
$ cargo clippy --locked --workspace --tests -- -D warnings
$ cargo fmt --all -- --check
```
