# コントリビューション

## コードをどこに置くか

構成は Laravel の MVC に従います。
Laravel のディレクトリにあたるものを、Rust のモジュールが担います。

| Laravel | ここ | 持つもの |
|---|---|---|
| `app/Models/` | `crates/yorishiro-core/src/models/` | レコードの型、入力DTO、そしてそれらを読み書きするクエリ |
| `app/Http/Controllers/` | `crates/yorishiro-server/src/http/controllers/` | リクエストが何を意味し、何をするか |
| `routes/` | `crates/yorishiro-server/src/routes.rs` | URLとコントローラの対応 |
| `app/Services/` | `crates/yorishiro-core/src/services/` | 1リクエストを超えて生きる判断: 認証、埋め込み、キュー |
| `resources/views/` | `ee/web/` | SPA |
| `database/migrations/` | `migrations/` | スキーマのバージョン管理 |
| `database/seeders/` | `crates/yorishiro-core/templates/*.json` | シードデータ |

有料版は上4つを `ee/crates/yorishiro-hosted/src/` に同じ形で持ちます。
そちらが追加するテーブルとエンドポイントのためです。

### モデルはテーブルを1つ持つ

型とクエリは同じモジュールに置きます。
Eloquent のモデルが両方を持つのと同じです。

リポジトリ層はありません。
`repositories` はパターンの名前であって層の名前ではなく、その名前のディレクトリを作った結果、`models` には構造体しか残らず、クエリは全部その隣にありました。

テーブルを足すなら `models/` にモジュールを足してください。
テーブルに触る判断を足すなら、判断を `services/` に、クエリを `models/` に置いてください。

### モデルではないもの

`migrations/`、`templates/*.json`、`db.rs` はデータベース自身の関心事であってモデルの関心事ではないので、意図的に `models/` の外にあります。

### クエリの場所

新しいテーブルのクエリは `models/` に置きます。
`models/` の外にある `sqlx::query` のうち、次は正しいものとして残します。

- `crates/yorishiro-core/src/db.rs` と `services/db_load_guard.rs` は接続の扱いであって、テーブルへのアクセスではありません
- `crates/yorishiro-server/src/http/controllers/health.rs` の `SELECT 1` は死活監視で、属するテーブルがありません
- `crates/yorishiro-core/src/services/auth/` はリクエストの身元を決める過程でキーを読みます。これはレコードではなく判断です

残りは意図ではなく既知の負債で、少しずつ移しています。
`crates/yorishiro-server/src/admin/commands.rs`、`http/controllers/setup/mod.rs`、`services/embedding/sync/`、
`ee/` 側の `services/marketplace.rs`、`official_templates.rs`、`tenant_auth.rs`、`oauth/users.rs`、`origin.rs`、`http/controllers/inference.rs` です。
すでにそのファイルを編集しているなら、クエリを `models/` に寄せてもらえると助かります。
そうでないなら触らないでください。
動作の変更と一緒に読めない移動だけの差分は、レビューできません。

## tests は src を1対1で写す

`tests/` は `src/` のツリーをそのまま再現します。
`crates/yorishiro-core/src/models/schemas/mod.rs` をテストするのは `crates/yorishiro-core/tests/models/schemas/mod.rs` だけです。

繋ぎ方は結合テストではなく include です。

```rust
#[cfg(test)]
#[path = "../../../tests/models/schemas/mod.rs"]
mod tests;
```

全クレートが `autotests = false` を設定しているので、`tests/` にあってどの `#[path]` からも名指されないファイルは、何にもコンパイルされません。
失敗はしません。ただ一度も走らないだけです。
ソースを移動したら、テストも同じように移動して `#[path]` の深さを直してください。

## push の前に

```sh
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo test --workspace
pnpm --dir ee/web run check   # ee/web を触ったときだけ
```

テストには `template1` に `vector` と `pg_trgm` が入った PostgreSQL と、**スーパーユーザではない**ロールが要ります。
スーパーユーザは `FORCE` の有無にかかわらず RLS を迂回するので、その権限で緑になっても分離については何も証明できません。

## このリポジトリの文章

1文1行で書きます。Markdown もコメントも同じです。
特定の文字数で折り返さないでください。ダッシュで2つの節を繋がないでください。

`migrations/` だけは見た目に関する規則すべての例外です。
sqlx がコメントを含めてファイル全体のチェックサムを取るので、適用済みのマイグレーションを編集するとサーバが起動しなくなります。
