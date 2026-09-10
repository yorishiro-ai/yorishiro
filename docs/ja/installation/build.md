# ソースからビルド

## 必要なもの

- Rust 1.97+（`rustup`推奨）
- Cargo
- PostgreSQL 開発用：`vector` と `pg_trgm` 拡張がインストールされた PostgreSQL インスタンス
- SQLite 開発用：`sqlite3`

## ビルド

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make build
```

## 実行

```console
$ cargo run -- start
```

## リリースビルド

```console
$ cargo build --release --bin yorishiro
```

バイナリは `target/release/yorishiro` に作成されます。

## PostgreSQL で実行

```console
$ DATABASE_URL=postgres://user:pass@host:5432/yorishiro cargo run -- start
```

## Valkey キューで実行

```console
$ DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
  YORISHIRO_QUEUE_KIND=Redis \
  QUEUE_URL=redis://host:6379 \
  cargo run -- start
```

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。
