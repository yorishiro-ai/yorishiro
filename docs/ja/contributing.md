# コントリビューションガイド

**English** | [日本語](../../CONTRIBUTING.md) | **日本語**

## コードレイアウト

| 置き場所 | 内容 |
|---|---|
| `src/models/` | レコードの構造体、入力DTO、クエリ |
| `src/controllers/` | リクエストハンドラ |
| `src/services/` | 認証、埋め込み、MCP |
| `migration/src/` | スキーママイグレーション |
| `src/tasks/` | 管理・単発コマンド |
| `ee/` | エンタープライズ版（`models/`、`controllers/`、`services/`をミラー） |

- `src/models/_entities/` は自動生成。手で編集しない。
- ビジネスロジックは生成されたエンティティの隣、`src/models/<table>.rs` に書く。
- 生SQLはSeaORMで表現できない場合のみ使う（JSONBの包含判定、pgvector、アドバイザリロック）。
- `src/db.rs` はテーブル操作ではなく接続処理。

## テーブルを追加する

1. `migration/src/` にマイグレーションを書く。
2. `make entities` で `_entities/` を再生成する。
3. `src/models/` 配下にモジュールを追加する。

## テスト

```console
$ make test
```

リクエストテストはクロージャを抜ける前に `close_app_pools` を呼ぶこと。パターンは `tests/requests/mod.rs` を参照。

## push前に

```console
$ make check          # cargo check --workspace
$ make clippy         # cargo clippy --workspace --tests -- -D warnings
$ make fmt-check      # cargo fmt --check
$ make test           # cargo test --workspace
```

テストには `template1` に `vector` と `pg_trgm` が入ったPostgreSQLと、スーパーユーザーでないロールが必要。

## Git ワークフロー

`develop` からブランチを切る。マージコミットを推奨。PRでは英語・日本語の両ドキュメントを更新する。

## ルール

コードスタイルのルールは `.claude/rules/` にある。
