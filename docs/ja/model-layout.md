# ソースの配置

このページは、ソースツリーが Loco 1.2.0 のジェネレーターの配置にどう従っているか、そして Loco が生成しない少数のディレクトリがなぜあるのかを説明します。

## 層

すべてのエントリポイントは、認証し、入力を解析し、モデルの操作を呼び出し、結果を返します。
ドメインロジックを持つことも、通常のクエリを組み立てることもしません。

| ディレクトリ | 役割 | Laravel での対応 |
| --- | --- | --- |
| `src/controllers/` | エントリポイント。REST ハンドラー、MCP ツール（`controllers/mcp/`）、リクエストミドルウェア（`controllers/middleware/`）、エクストラクター | Controllers、Middleware |
| `src/tasks/` | エントリポイント。コマンドライン | Console commands |
| `src/dtos/` | リクエストとレスポンスの転送型。コントローラーごとに 1 ファイル | Resources、Form requests |
| `src/models/` | テーブルごとの拡張（`<table>.rs`）、生成された `_entities/`、複数テーブルにまたがるモデル | Eloquent models |
| `src/workers/` | バックグラウンドジョブと、それらが共有するキューのライフサイクル | Jobs |
| `src/initializers/` | プロセスの存続期間にわたって動く処理 | Service providers |
| `src/data/` | 型付き設定、設定の読み込み、組み込みデータ | Config |
| `src/fixtures/` | シードデータ | Seeders |
| `src/services/` | 外部クライアント（embedding プロバイダー）のみ。Loco に置き場がないため | Services |
| `src/app.rs` | コミュニティ版の組み立て。Loco の `Hooks` の各メソッドに 1 関数ずつ対応し、エディションが拡張に使う型も提供します | Application bootstrap |
| `edition/` | 組み立ての起点。`src/` の外にある、唯一の `Hooks` 実装 | Service container |

`ee/` は `src/` に重ねる層です。
コミュニティ版のフォルダー階層とファイル名をそのまま踏襲するので、コミュニティ版のパスから、対応するエンタープライズ版の場所が分かります。
`ee/services/` にも同じ制約を適用します。ここに残すのは外部の LLM・OAuth クライアントだけで、認証、ライセンスミドルウェア、プランデータ、永続化は、それぞれ対応する controllers、data、models 配下に置きます。

## エディションの組み立て

`src/` はエンタープライズ層を参照しません。
`cfg(feature = "enterprise")` も `crate::ee` も、エンタープライズ層の名前も置きません。
`scripts/check_edition_boundary.py` が `src/` 以下のすべての Rust ファイルを文字列として検査し、`make check-all`、`make check-all-ee`、CI で実行されます。

このクレートの `Hooks` 実装は `edition/app.rs` の `edition::App` だけです。
各フックは `src/app.rs` の対応する関数を呼び、その結果に有効なエディションが追加します。
有効なエディションは、`enterprise` 機能が有効なら `ee/app.rs`、そうでなければ `edition/community.rs` です。
どちらも同じ関数を公開するので、シグネチャが食い違うと、遅れた側のエディションのビルドが失敗します。
両者の間にトレイトは置きません。重ねる層が 1 つだけで、実行時に選ぶ余地がないためです。

`src/` は、重ねる層が使うための小さくエディション中立な入力を公開します。

| 入力 | エディションの使い方 |
| --- | --- |
| `workers::registry::WorkerRegistry` | `register` でワーカー型を追加します。タグとキューはワーカー型自身が宣言した値を使います。 |
| `app::RouteMounts` | ルートグループとその OpenAPI ドキュメントをマウントし、IP ごとのレート制限を共有する認証情報のパスを指定します。 |
| `controllers::mcp::McpToolSet` | MCP ツールと、リクエストがどのツールを使えるかを決めるポリシーを追加します。 |
| `shared_store` の接続点 | 認証やキューポリシーなど、独自の `Arc<dyn Trait>` を挿入して実行時に規則を置き換えます。 |

ワーカーのタグ、Redis のキュー設定、Loco へのワーカー登録は、すべて合成された 1 つの `WorkerRegistry` から導出します。
そのため `yorishiro worker-tags` の出力が、プロセスが登録する内容とずれることはありません。

テーブルは、重ねる層だけが使う場合でも `src/models/` にモジュールを残します。
マイグレーションと生成エンティティは 1 つの共有スキーマであり、すべての生成エンティティはどのビルドでも `ActiveModelBehavior` 実装（秘密情報を含むものは値を隠す `Debug` も）を必要とするためです。
重ねる層がそのテーブルについて持つ振る舞いは、`ee/models/` の対応するファイルに置きます。

## 生成コードとの境界

`migration/src/` のマイグレーションは手書きでデータベーススキーマを定義します。
`cargo loco db entities` は SeaORM エンティティを `src/models/_entities/` 以下に生成します。
生成ディレクトリはその場所に残し、手で編集してはいけません。
Loco 1.2.0 のジェネレーターは出力先 `src/models/_entities` を固定しています。

テーブルごとのアプリケーション所有の拡張は `src/models/<table>.rs` にあります。ジェネレーターも、拡張がなければここに書き出します。
大きなモデルは、`src/models/<table>.rs` から宣言する非公開の実装モジュールを `src/models/<table>/` に置けます。
1 つのテーブルが所有するファインダーと永続化はその拡張に置き、間にリポジトリ層は挟みません。
`tenancy`、`search`、`entity_embeddings` のように複数テーブルにまたがるモデルは、テーブルのモデルと同じ階層に置きます。

チェックイン済みの `Makefile` の `entities` ターゲットは、使い捨て PostgreSQL にマイグレーションを適用して `cargo loco db entities` を実行します。
CI は 10 個以上のエンティティファイルが生成されることと、生成結果がチェックイン済みツリーと一致することを検証します。
3 つの pgvector エンティティファイルは必要な `ActiveModelBehavior` 実装を SeaORM codegen が省略するため、比較前に復元します。

## 秘密情報を含む生成モデル

SeaORM 2.0.4 には、生成される `Debug` derive をテーブルごとに置き換える設定がありません。
そのため `make entities` はコード生成後に `scripts/harden_generated_entities.py` を実行します。
この後処理は明示的な疑わしい名前のポリシーを使います。
認証情報を表す完全一致名と、`_api_token`、`_service_key`、`_password`、`_authorization` などの認証情報サフィックスを保護し、`key` や `*_hash` のような一般的な名前は対象にしません。
モデルの derive から `Debug` を削除し、疑わしいフィールドすべてに `serde(skip_serializing)` を追加します。
疑わしいフィールドが対応していない生成形式に現れた場合は、露出を黙って許可せず後処理を失敗させます。
アプリケーションが所有する `src/models/workspace_llm_keys.rs` と `src/models/workspace_embedding_keys.rs` は `api_key` を含めない安全な `Debug` 実装を提供し、生成されたクエリと書き込みの型はそのまま維持します。
`_entities` は手で編集せず、生成コマンドを変更する場合も生成後のハードニング処理をエンティティのワークフローに残してください。

## 静的データ

組み込みテンプレートの JSON アセットは `data/templates/` に置き、`src/data/templates.rs` から埋め込みます。
これは生成エンティティではなくスキーマ定義なので、ソースアセットとしてレビューとテストを行います。
