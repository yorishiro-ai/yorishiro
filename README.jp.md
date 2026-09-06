# Yorishiro (依り代)

**[English](README.md)** | 日本語

自分のデータ構造を定義し、キーワードだけでなく意味で検索できるナレッジストア。

## できること

- 自分のデータ構造を定義する
  - フィールド、バリデーションルール、エンティティ間のリレーションを持つエンティティ型を作成できます。データモデルが変わってもコード修正は不要。
- 意味で検索する
  - 「配送に関する顧客クレーム」といった説明を入力すると、正確に同じ言葉がなくても関連するレコードが見つかります。
- 全文検索
  - 埋め込みがまだ生成されていないレコードもキーワード検索でカバーします。
- AIツールに接続
  - Claude、Cursor、他のMCP対応クライアントが23個の組み込みツールを使ってデータを検索・作成・管理できます。
- REST API
  - 標準HTTPエンドポイントで自動化できます。
- バージョン復元
  - スナップショットで個別レコードを以前の状態に復元できます。
- チームで整理する
  - テナントとワークスペース、ロールベースのアクセス制御でデータを適切に分割・共有できます。

## アーキテクチャ

```mermaid
flowchart TD
    MCPClient["MCPクライアント<br/>(Claude, Cursor など)"]
    RESTClient["RESTクライアント<br/>(curl, SDK, ブラウザ)"]

    subgraph Server["Yorishiro（axum）"]
        MCP["MCP<br/>(23ツール)"]
        REST["REST API"]
        Core["コア + ee/<br/>(スキーマ / エンティティ / 検索 / 認証 / marketplace / billing / OAuth / LLMキー)<br/>ライセンスなしのルートは404を返す"]
    end

    DB[("PostgreSQL<br/>または SQLite")]

    MCPClient -->|"MCPツール"| MCP
    RESTClient -->|"HTTP API"| REST
    MCP --> Core
    REST --> Core
    Core --> DB
```

## エディション

| | フリー | エンタープライズ |
|---|---|---|
| コア機能 | 利用可能 | 利用可能 |
| エンタープライズ機能（marketplace、ビルド、OAuth、LLM推論） | 利用不可 | 利用可能 |

1つのバイナリ、1つのリポジトリ。エンタープライズ機能はライセンスキーで有効化します。

## クイックスタート

デフォルト設定はSQLiteを使用しており、外部データベースは不要です。

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. `http://localhost:8080/` にアクセスし、セットアップウィザードでアカウントを作成。
2. APIキーを生成し、エンティティの作成を開始。

### PostgreSQL に切り替える

マルチテナントホスティングやベクトル検索には PostgreSQL を使用：

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

### Valkey (Redis) に切り替える

分散キュー処理には Valkey（または Redis）を使用：

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e YORISHIRO_QUEUE_KIND=Redis \
    -e QUEUE_URL=redis://user:pass@host:6379 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

ソースからビルドする場合：

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
```

## 設定

設定は環境変数で行います：

| 変数 | デフォルト | 説明 |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc` | マルチテナントまたはベクトル検索には `postgres://` を指定 |
| `YORISHIRO_MAX_TENANTS` | `1` | テナント上限（`0` = 無制限） |
| `YORISHIRO_LICENSE_KEY` | *(空)* | エンタープライズライセンスキー |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | 埋め込みバックエンド（`none` で無効） |
| `YORISHIRO_QUEUE_KIND` | 自動検出 | Valkey/Redis キューには `Redis` を設定 |
| `QUEUE_URL` | `DATABASE_URL` と共有 | Redis キューに必須 |

全一覧は [docs/ja/configuration.md](docs/ja/configuration.md) を参照してください。

## ドキュメント

| ドキュメント | 内容 |
|---|---|
| [docs/ja/installation.md](docs/ja/installation.md) | Docker、Docker Compose、deb/rpm、ソースからビルド |
| [docs/ja/configuration.md](docs/ja/configuration.md) | 全設定：埋め込み、検索クォータ、ログ、キューバックエンド |
| [docs/ja/sqlite.md](docs/ja/sqlite.md) | SQLiteモード：機能と制限 |
| [CONTRIBUTING.md](docs/ja/CONTRIBUTING.md) | 貢献ガイドライン |
| [AGENTS.md](docs/ja/AGENTS.md) | AIエージェント向け情報 |

## ライセンス

[Business Source License 1.1](LICENSE) の下でライセンスされています。セルフホスティングは許可されています。2030-07-14 にこのバージョンはGNU General Public License Version 2.0以降に自動的に変換されます。
