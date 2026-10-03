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

`ee/` は `src/` に重ねる層です。
コミュニティ版のフォルダー階層とファイル名をそのまま踏襲するので、コミュニティ版のパスから、対応するエンタープライズ版の場所が分かります。
`ee/services/` にも同じ制約を適用します。ここに残すのは外部の LLM・OAuth クライアントだけで、認証、ライセンスミドルウェア、プランデータ、永続化は、それぞれ対応する controllers、data、models 配下に置きます。

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
