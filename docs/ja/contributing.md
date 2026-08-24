# コントリビューションガイド

## コードをどこに置くか

構成は標準的なMVCの分割に倣い、そのディレクトリの役割をRustのモジュールが担う。

| 置き場所 | 持つもの |
|---|---|
| `src/models/` | レコードの形、入力DTO、それらを読み書きするクエリ |
| `src/controllers/` | リクエストが何を意味し、何をするか |
| `src/app.rs`の`Hooks::routes` | URLとコントローラの対応 |
| `src/services/` | 1リクエストより長く生きる判断: 認証、埋め込み、MCP |
| `migration/src/` | スキーマのバージョニング |
| `src/app.rs`の`Hooks::seed`、`templates/*.json` | 初期データ |
| `src/tasks/` | 管理・単発コマンド(`cargo loco task`経由) |

有償版は、追加するテーブルとエンドポイントについて、models・controllers・servicesを`ee/crates/yorishiro-hosted/src/`配下に同じ形で持つ。

### モデルはテーブルを1つ持つ

その2つは同じモジュールに置く。
リポジトリ層は無い。
`repositories`はパターンの名前であって層の名前ではなく、その名前のディレクトリを作った結果、`models`が構造体だけを持ち、クエリはすべて隣に置かれる状態になった。

`src/models/_entities/`は`cargo loco db entities`が生成するもので、手で編集しない。
ビジネスロジックは同じ階層の`src/models/<table>.rs`に置く。

テーブルを追加するときは、`migration/src/`にマイグレーションを追加し、エンティティを再生成し、`models/`配下にモジュールを追加する。
テーブルを参照する判断を追加するときは、判断を`services/`に、クエリを`models/`に置く。

### モデルでないもの

`migration/src/`、`templates/*.json`、`db.rs`はデータベース側の関心事であってモデルの関心事ではないため、意図的に`models/`の外に置いている。

### クエリの置き場所

新しいテーブルのクエリは`models/`に置く。
生SQLは、SeaORMのエンティティAPIで表現できないものに限る。
具体的にはJSONBの包含判定(`data @> filter`)、pgvectorの類似検索、アドバイザリロック、どのエンティティも列を持たない値(同一クエリ内で計算する相関集計)、そして1つの文として実行されることに正しさが依存する書き込みである。
それ以外は、呼び出し側が持っているコネクションまたはトランザクションの上で`Entity::find()`/`ActiveModel`/`Set(...)`を使う。
最後の2つの現在の実例は`ee/crates/yorishiro-hosted/src/models/marketplace.rs`にある。
`list_marketplace`は`identity_templates`自体の列に加えて3つの相関サブクエリを選択しており、これはどのエンティティ射影からも作れない。
`insert_next_version`はテンプレートの次のバージョン番号を計算する`INSERT ... SELECT`を1つの文として実行しており、これによってアドバイザリロックが2つの同時公開を同じバージョン番号へ競合させないという保証が成り立っている。

`models/`の外にある生SQLのうち、以下は正しいものとして残す。

- `src/db.rs`はテーブルアクセスではなくコネクション管理であり、その`SET ROLE`/`RESET`はプールのセッションライフサイクルそのものである。
- `src/services/auth/authenticate.rs`は、リクエストの識別を決める一部としてキーを読む。
  これはレコードではなく判断である。
  `last_used_at`の書き込みも同じ判断の一部で、リクエストのトランザクションではなく専用の短命なコネクションで意図的に実行している。
  リクエストのトランザクションに載せると、読み取り専用のリクエストや拒否されたリクエストのたびにロールバックされてしまうためである。
- `src/services/embedding/sync.rs`と`src/tasks/resync_embeddings.rs`は`embedding`カラムを書き・走査する。
  このカラムはpgvector型で、エンティティAPIでの表現手段が無い。
- `ee/crates/yorishiro-hosted/src/services/official_templates.rs`はシーダーであり、`migration/src/`を`models/`の外に置くのと同じ対応関係でモデルの外に位置する。
- `ee/crates/yorishiro-hosted/src/services/tenant_auth.rs`は認証の例外の有償版側の対応物である。
  `TenantScopedAuthenticator::authenticate`はこのクレートの`Authenticator`実装であり、`create_tenant_api_key`は`create_api_key`にテナント存在チェックとロール上限チェックを畳み込んだもので、どちらもレコードではなく判断である。

`ee/crates/yorishiro-hosted/src/controllers/inference.rs`には`models/`に置くべきクエリが残っており、まだ移していない。
このファイルを別の用件で編集しているなら、クエリを移すのは歓迎する。
そうでないなら、そのままにしておく。
ファイルを触る理由が他に無い移動は、挙動の変更と突き合わせてレビューできない差分になる。

### MCPはコントローラではなくサービス

`src/services/mcp/`には、サーバ型(`YorishiroMcpServer`)とドメインごとの`#[tool_router]`実装が置かれている。
MCPツールはコントローラと同じくエントリポイントであるにもかかわらず、である。
これは上の対応表に対する意図的な例外で、MCPはルーティングのロジックではなく`ee/`が合成する接合面だからである。
ルートの*マウント*は`src/controllers/mcp.rs::mount()`の1行で、`Hooks::after_routes`から呼ばれる。
`controllers/`に属するのはその1行のほうである。

## テスト

`tests/`は素のLoco統合テストクレートである。
`#[path]`インクルードも`autotests = false`も無い。
`tests/`配下のファイルは、そこにあるという理由でコンパイルされ実行される。

リクエストテストは`loco_rs::testing::request::request_with_create_db::<App, _, _>`でアプリを起動する。
**そのすべてが、クロージャを抜ける前に`close_app_pools`を呼ばなければならない。**
呼ばないと、テストが通っていてもティアダウンでpanicする。
アプリはLocoのハーネスが把握していないプールを開いており、そのどれも自力では閉じないためである。
`tests/requests/mod.rs`にそのヘルパーがあり、これが踏襲すべきパターンである。

`ee/`では、共有テストヘルパーは`tests/lib.rs`で1度だけ宣言し、`use crate::tests::test_helpers;`で参照する。
複数のファイルで`mod test_helpers;`を宣言すると`clippy::duplicate_mod`に引っかかる。

## push前に

```sh
make check          # cargo check --workspace
make clippy         # cargo clippy --workspace --all-targets -- -D warnings
make fmt-check      # cargo fmt --check
make test           # cargo test --workspace
```

`make -C <path> <target>`はどこからでも実行できる。
`cargo loco task`とCLIバイナリが「`config/`がプロセス自身の作業ディレクトリにあること」を要求するため、これが効いてくる。

テストスイートには、`vector`と`pg_trgm`が**`template1`に**入ったPostgreSQLが要る。
デプロイ本体のデータベースに入っているだけでは足りない。
Locoのハーネスは使い捨てのテスト用データベースを`CREATE DATABASE`で作り、これは`template1`を複製するためである。
思い込まずに`psql -d template1 -c '\dx'`で確認すること。

さらに、スーパーユーザー**でない**ロールが要る。
スーパーユーザーは`FORCE`の有無にかかわらずRLSを迂回するため、その権限で緑になっても分離については何も証明しない。

## このリポジトリの文章

Markdownでもコメントでも、1文1行。
ハードラップしない。
2つの節をダッシュでつながない。

`migration/src/`はこの規則の例外ではない。
`sea-orm-migration`は、このプロジェクトが以前使っていたsqlxのマイグレーションと違い、マイグレーションファイルのチェックサムを取らないため、適用済みのマイグレーションを編集しても問題ない。
