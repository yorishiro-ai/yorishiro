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
`repositories`はパターンの名前であって層の名前ではない。その名前のディレクトリを別に作ると、`models`は構造体だけを持つ空箱になり、クエリはすべて隣に追いやられてしまう。

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
ただし対象は実際にそれを必要とする個々のクエリに限る。ファイル全体を対象にするわけではない。
同じファイルの中にエンティティAPIで書くべき別のクエリがあってもよく、実際にある。

- `src/db.rs`はテーブルアクセスではなくコネクション管理であり、その`SET ROLE`/`RESET`/`set_config`/アドバイザリロックはSeaORMに対応物の無いPostgresのセッションプリミティブである。
- `src/services/auth/authenticate.rs`の`authenticate_api_key($1)`呼び出しは、RLSが意図的に迂回を許す唯一の口であるSECURITY DEFINER関数で、通常のコネクションのセッション状態では隠れてしまう行を読む。
  `touch_last_used`の`last_used_at`書き込みも同じ種類のコネクション(リクエストのRLSセッション変数がスコープされた`TenantDb::acquire_for_workspace`が返す、汎用のコネクションではないもの)で動く必要があるため、同じく生SQLのままにしている。
  他のコネクションでエンティティAPI経由の更新を試みると、読み取り専用リクエストのトランザクションと一緒にロールバックされるか、`identity_api_keys`自体のRLSポリシーの下で黙って0行を更新することになる。
  この形に当てはまらない`authenticate.rs`の他の処理はエンティティAPI上にある(`authenticate_sqlite`/`touch_last_used_sqlite`を参照。これらはRLSを保持する必要が無いのでSQLite上では全面的にエンティティAPIを使っている)。
- `src/services/embedding/sync.rs`と`src/tasks/resync_embeddings.rs`は`embedding`カラムを書き・走査する。
  このカラムはpgvector型で、エンティティAPIでの表現手段が無い。
- `ee/crates/yorishiro-hosted/src/services/tenant_auth.rs`の`TenantScopedAuthenticator::authenticate`は、同じSECURITY DEFINER関数の2引数オーバーロードである`authenticate_api_key($1, $2)`を呼んでおり、上と同じ理由で残している。
  `create_tenant_api_key`自身のテナント存在チェック・ロール取得・INSERTにはその理由が無く、エンティティAPI上にある。

### MCPはコントローラではなくサービス

`src/services/mcp/`には、サーバ型(`YorishiroMcpServer`)とドメインごとの`#[tool_router]`実装が置かれている。
MCPツールはコントローラと同じくエントリポイントであるにもかかわらず、である。
これは上の対応表に対する意図的な例外である。MCPはルーティングのロジックというより、`ee/`が合成する接合面という性格が強い。
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
