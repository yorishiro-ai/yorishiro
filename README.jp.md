# Yorishiro (依り代)

**[English](README.md)** | 日本語

MCPネイティブのマルチテナント・ナレッジストア。ユーザー定義スキーマをサポートします。

エンティティの「型」（フィールド、制約、リレーション）をJSONメタスキーマとして定義し、そのスキーマに検証されたデータをREST APIとMCP（Model Context Protocol）の両方を通じて読み書きできます。`x-embed` 付きのフィールドは自動的にベクトル埋め込みされ、自然言語クエリによる類似度検索が可能になります。

## できること

- **スキーマ駆動データ**。フィールド、バリデーションルール、エンティティ間のリレーションを自由に定義できます。
- **MCP統合**。Claude、Cursor、または任意のMCP互換クライアントを、検索・作成・管理用の23個の組み込みツールで接続できます。
- **REST API**。APIキー認証付きの標準HTTPエンドポイント。
- **ベクトル検索**。埋め込み済みテキストフィールド全体の自然言語による類似度検索。
- **全文検索**。埋め込みのないエンティティを含む全エンティティの全文検索。
- **スナップショットと復元**。ワークスペーススナップショットによる時点復元。
- **マルチテナンシー**。ロールベースアクセス（owner、admin、member、viewer）付きのテナント・ワークスペースを整理。

## アーキテクチャ

```mermaid
flowchart TD
    MCPClient["MCPクライアント<br/>(Claude など)"]
    RESTClient["RESTクライアント<br/>(curl/SDK)"]

    subgraph Enterprise["ee/（enterprise edition）"]
        EnterpriseREST["marketplace / origin / ビルディング / OAuth / LLMキー"]
    end

    subgraph Server["yorishiro（axum）"]
        Core["コア<br/>(スキーマ / エンティティ / 検索 / 認証)"]
        MCPAdapter["MCPアダプタ<br/>(23ツール)"]
        RESTAdapter["RESTアダプタ"]
        MCPAdapter --> Core
        RESTAdapter --> Core
    end

    DB[("PostgreSQL 18 + pgvector<br/>(RLS分離)")]
    SQLiteDB[("SQLite（単一テナント、sqlite-vec）")]

    MCPClient -->|"/mcp"| MCPAdapter
    RESTClient -->|"/api/*"| RESTAdapter
    RESTAdapter --> Core
    EnterpriseREST --> Core
    Core --> DB
    Core --> SQLiteDB
```

## エディション

| | ライセンスキーなし | `YORISHIRO_LICENSE_KEY` あり |
|---|---|---|
| コア機能 | 提供 | 提供 |
| エンタープライズ機能（marketplace、ビルド、OAuth、LLM推論） | `404` | 提供 |
| Web UI | どちらでも提供 | |

1つのリポジトリ、1つのバイナリ。設定の有無で動作が変わります。ライセンスチェックはリクエストごとに実行されるため、期限切れでも再起動なしでエンタープライズ機能が停止します。

## クイックスタート

最も簡単な方法はDockerを使用することです。モデルのダウンロードや手動設定は不要です。

1. サーバーを起動:

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
       ghcr.io/yorishiro-ai/yorishiro:latest
   ```

2. `http://localhost:8080/` にアクセスし、セットアップウィザードで所有者アカウントを作成。

3. APIキーを生成し、エンティティの作成を開始。

ソースからビルドする場合:

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
```

これによりDocker ComposeでPostgreSQLとアプリが起動します。

## 設定

環境変数でランタイムの動作を制御します。一般的な設定:

| 変数 | デフォルト | 説明 |
|---|---|---|
| `DATABASE_URL` | *(必須)* | PostgreSQL接続文字列 |
| `YORISHIRO_MAX_TENANTS` | `1` | テナント上限（`0` = 無制限） |
| `YORISHIRO_LICENSE_KEY` | *(空)* | エンタープライズライセンスキー |
| `YORISHIRO_EMBEDDING_PROVIDER` | *(空)* | 埋め込みバックエンド（`local` はローカルモデル） |

全一覧は [docs/configuration.md](docs/configuration.md) を参照してください。

## SQLiteモード

Yorishiroはローカル評価・単一テナントの個人利用のためにSQLiteで動作できます。sqlite-vecによるベクトル検索も利用可能です。マルチテナントホスティングはサポートしていません（データベースレベルの分離がないため）。

詳細は [docs/sqlite.md](docs/sqlite.md) を参照してください。

## ドキュメント

| ドキュメント | 内容 |
|---|---|
| [docs/configuration.md](docs/configuration.md) | 全環境変数：埋め込み、検索クォータ、ログ設定 |
| [docs/sqlite.md](docs/sqlite.md) | SQLiteモード：機能と制限 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | コードレイアウト、テスト、push前のチェック |
| [AGENTS.md](AGENTS.md) | AIエージェントの焦点ルール |

未執筆: メタスキーマガイド、REST/MCP APIリファレンス、デプロイメントガイド。

## ライセンス

[Business Source License 1.1](LICENSE) の下でライセンスされています。セルフホスティングは許可されています。唯一の制限はYorishiroを競合のホストサービスとして提供することです。2030-07-14 にこのバージョンはGNU General Public License Version 2.0以降に自動的に変換されます。
