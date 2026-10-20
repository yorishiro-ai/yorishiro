# ソースからビルド

## 必要なもの

- サポート対象の MSRV は Rust 1.94 です。本番向けの通常ビルドは固定した Rust 1.97 ツールチェーンを使います（`rustup` 推奨）。
- Cargo
- EnterpriseのJWTバックエンド（AWS-LC）をビルドするCコンパイラ。対応するネイティブターゲットでは、crate同梱の生成済みバインディングを使います。
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

EnterpriseのライセンスとOIDCのRS256検証は、`jsonwebtoken`のRustCrypto RSAではなくAWS-LCバックエンドを使います。
RUSTSEC-2023-0071を抑制するのではなく、対象の`rsa`依存を削除しています。
許可するRS256アルゴリズムとライセンス・OIDCのclaim検証は変更しません。

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
