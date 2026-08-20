# Yorishiro(依り代)

[English](../../README.md) | **日本語**

ユーザー定義スキーマを持つ、MCPネイティブなマルチテナント・ナレッジストア。

エンティティの「型」(フィールド・制約・リレーション)を利用者がJSONメタスキーマとして定義し、そのスキーマで検証されたデータをREST APIとMCP(Model Context Protocol)の両方から読み書きできます。
`x-embed`を付けたフィールドは自動でベクトル埋め込みされ、自然文クエリによる類似検索ができます。

## アーキテクチャ

```mermaid
flowchart TD
    MCPClient["MCPクライアント<br/>(Claude等)"]
    RESTClient["RESTクライアント<br/>(curl/SDK)"]

    subgraph Paid["ee/ (有償版。同じバイナリに composeされる)"]
        HostedMCP["HostedMcpServer<br/>(自身のツールの後、委譲する)"]
        HostedREST["本版のルート<br/>(マーケットプレイス / 出自 / 課金 / OAuth)"]
    end

    subgraph Server["yorishiro-server (axum)"]
        MCPAdapter["MCPアダプタ<br/>(YorishiroMcpServer、23ツール)"]
        RESTAdapter["RESTアダプタ"]
        Core["yorishiro-core<br/>(schemas / entities / relations /<br/>search / auth / embedding)"]
        MCPAdapter --> Core
        RESTAdapter --> Core
    end

    DB[("PostgreSQL 18 + pgvector<br/>(identity/contentスキーマ、RLSによる分離)")]

    MCPClient -->|"/mcp"| HostedMCP
    RESTClient -->|"/api/*"| HostedREST
    HostedMCP -->|"委譲"| MCPAdapter
    HostedREST -->|"フォールバック"| RESTAdapter
    HostedREST --> Core
    Core --> DB
```

コミュニティ版バイナリ(`yorishiro-ce-server`)は内側のサブグラフ単体である。
同じAPIルートを、`ee/`を前段に置かずに提供する。
SPAは`ee/`にあるため、Web UIは提供しない。

- cargo workspace
  - `yorishiro-core`(ドメインロジック)と`yorishiro-server`(HTTPサーバ・アダプタ層)で構成されます。
  - リポジトリ層を持ちクエリを発行するのは`yorishiro-core`であり、`yorishiro-server`はそれをHTTPとMCPへ橋渡しします。
- 2階層のテナント構造
  - **テナント**(組織/アカウント)は複数の人間の**ユーザー**をowner/admin/member/viewerのロールで紐付けられ、複数の**ワークスペース**を持ちます。
  - 全てのコンテンツ(スキーマ/エンティティ/リレーション)とAPIキーはちょうど1つのワークスペースに属します。
  - これにより1つの組織内で複数の独立したプロジェクト(本番/ステージング、チームごとのワークスペースなど)をテナントを分けずに運用でき、複数人で同一テナントの管理権限を共有できます。
- RLSによる分離
  - 全テーブルにPostgreSQLのRow Level Securityを適用します。
  - リクエストごとにAPIキーからワークスペース(とその所属テナント)を解決し、セッション変数`app.current_tenant`/`app.current_workspace`を設定したコネクションでのみデータへ到達できます。
  - アプリは専用ロール(`yorishiro_app`、`BYPASSRLS`なし)で動作し、制御プレーンのテーブル(`identity.tenants`/`identity.users`/`identity.tenant_memberships`)にはこのロールから一切アクセスできません。
    これらはマイグレーションロールのプール経由で操作します。
    管理CLIに加え、サインアップとセットアップのエンドポイントも同じ経路です。
    RLSがスコープするためのテナント/ワークスペースが、まだ存在しない段階で動くためです。

  1つのプロセスが同じデータベースへ2つのプールを持ち、どちらを通るかで到達できる範囲が決まります。

```mermaid
flowchart LR
    Req["リクエスト<br/>(APIキーがワークスペースを解決)"]
    Admin["管理CLI / サインアップ / セットアップ"]

    subgraph Pools["1プロセス、2プール"]
        Tenant["tenant_db<br/>SET ROLE yorishiro_app<br/>+ app.current_tenant / _workspace"]
        Identity["identity_pool<br/>マイグレーションロール、SET ROLEなし"]
    end

    Content[("content.*<br/>ワークスペース単位でRLSが効く")]
    Control[("identity.tenants / users / memberships<br/>yorishiro_appにGRANTが無い")]

    Req --> Tenant
    Admin --> Identity
    Tenant --> Content
    Tenant -. "permission denied" .-> Control
    Identity --> Control
```

  点線が要点です。
  リクエストは制御プレーンをクエリしても読めません。
  ロールがそれらのテーブルにGRANTを持たないためです。
- クォータ
  - テナントの`max_workspaces`とワークスペースの`max_entities`は、それぞれワークスペース作成時・エンティティ作成時に強制されます。
  - どちらもデフォルトは`NULL`(無制限)で、運用者がテナント/ワークスペースごとに明示的な上限を設定できます。
- スキーマバージョニング
  - 同名スキーマの再登録は新バージョンとして追加され、破壊的変更(フィールド削除・型変更・必須化など)は差分として報告されます。
  - 既存エンティティは作成時点のスキーマバージョンに対して検証され続けます。

  バージョン発行時に書き換えは起きないため、昨日書いたエンティティは書いた当時の規則のまま残ります。

```mermaid
flowchart TD
    V1["スキーマ v1<br/>archived"]
    V2["スキーマ v2<br/>active"]

    E1["エンティティA<br/>schema_version = 1"]
    E2["エンティティB<br/>schema_version = 2"]

    V1 -->|"create_schema が v1 をarchiveし<br/>v2 をactiveにする"| V2
    E1 -.->|"検証は引き続き"| V1
    E2 -->|"検証は"| V2

    New["新規エンティティ"] --> V2
```

  バージョンの発行は安価かつ非破壊です。
  一括の書き換えは走らず、既存の行が無効になることもありません。
- 単一バイナリ
  - 上記は全て単一の`yorishiro-server`バイナリに含まれており、既定でシングルテナント構成(`YORISHIRO_MAX_TENANTS=1`)として動作します(無制限にするには`0`を設定)。
  - この上限は初回セットアップウィザード(`/`のブラウザUI、または`POST /setup`)も有効にし、テナント・ワークスペース・ownerアカウントを一括作成できます。
    管理CLIは不要です。
  - 最初のアカウント以降は招待制のみ(`admin create-invite` → `POST /auth/signup` → `POST /auth/login`)です。
  - テナントのowner/adminは管理CLIを使わず、メンバー(`/api/members`)とワークスペース(`/api/workspaces`)をREST経由または同じブラウザUIから管理できます。

## クイックスタート

詳しいガイドは[docs/ja/setup.md](setup.md)を参照してください(ビルド済みバイナリでの起動、systemdでのバックグラウンド運用を含みます)。
最短経路はDockerです。

1. 埋め込みモデルを取得します(既定のローカルONNXプロバイダは外部サービスを必要としません)。

   ```console
   $ mkdir -p models
   $ curl -L -o models/model.onnx \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/onnx/model_quantized.onnx
   $ curl -L -o models/tokenizer.json \
       https://huggingface.co/Xenova/multilingual-e5-large/resolve/main/tokenizer.json
   ```

2. サーバを起動します。

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -v "$(pwd)/models:/app/models:ro" \
       -e DATABASE_URL=postgres://... \
       ghcr.io/yotsunagi/yorishiro:latest
   ```

   これだけでシングルテナント構成として完全に動作します。
3. `http://localhost:8080/`にアクセスし、セットアップウィザードでownerアカウントを作成します。

ソースからビルドする場合は、リポジトリをcloneして手順1と同様にモデルファイルを配置した後、`make init`(Docker Compose、makeが必要)がPostgreSQLとアプリを起動します。

```console
$ git clone https://github.com/yotsunagi/yorishiro && cd yorishiro
$ make init
```

## エディション

1リポジトリ、1イメージ、2バイナリです。
どちらを動かすかで、ディスク上に何があるかが決まります。
設定で切り替えるものではありません。

| | `yorishiro-server` | `yorishiro-ce-server` |
|---|---|---|
| 含むもの | `ee/`を含めたすべて | BUSL-1.1のみ。`ee/`の痕跡なし |
| 有償機能 | `YORISHIRO_LICENSE_KEY`で有効化。無ければ`404` | 存在しない |
| Web UI | バイナリから配信 | 無し。`/`は`404` |
| ライセンス | [BUSL-1.1](../../LICENSE)、`ee/`は[`ee/LICENSE`](../../ee/LICENSE) | [BUSL-1.1](../../LICENSE) |

既定の成果物は`yorishiro-server`で、ライセンスキーが無ければ有償のAPIサーフェスが`404`を返します。
ただしコミュニティ版と同一ではありません。
`ee/`はディスク上にあり、SPAはライセンスで塞がないためWeb UIはどちらでも提供されます。
`yorishiro-ce-server`は、プロプライエタリなコードをディスクに置けない配備のためにあります。
配布方針、再配布の要件、設定ではなくパッケージを読む監査といった事情です。

有償側は[`ee/README.md`](../../ee/docs/ja/README.md)が自分で説明します。

## ドキュメント一覧

| ドキュメント | 内容 |
|---|---|
| [docs/ja/setup.md](setup.md) | セットアップ手順一式(起動・エンドポイント・テナント/ワークスペース/ユーザー/APIキー発行・認証とscope) |
| [docs/ja/schema.md](schema.md) | エンティティ型・リレーションを定義するメタスキーマガイド |
| [docs/ja/api.md](api.md) | REST APIとMCPツールのリファレンス |
| [docs/ja/embedding-providers.md](embedding-providers.md) | 埋め込みプロバイダの設定(ローカル`local` ONNX / `openai`互換) |
| [docs/ja/configuration.md](configuration.md) | 環境変数/`config.yml`リファレンス |
| [docs/ja/deployment.md](deployment.md) | 本番デプロイ手順 |
| [docs/ja/operations.md](operations.md) | 運用上の注意(バックアップ・レート制限・可観測性) |
| [docs/ja/contributing.md](contributing.md) | コードをどこに置くか、`tests/` が `src/` を写す仕組み、push前に走らせるもの |

## 開発

日々の開発コマンドは、`app`とは別の`dev`サービス(Rustツールチェーン、`make up`では起動されず必要な時だけ起動)経由で実行します。

```console
$ make fmt-check
$ make clippy
$ make test
$ make shell   # cargo/psql/sqlx-cliへの単発アクセス
```

`models/`にONNXモデルを置くと、実モデルでの埋め込み統合テストが有効になります(無い場合は自動スキップ)。

## ライセンス

[Business Source License 1.1](../../LICENSE)。
自己ホスティング(商用・社内利用を含む)は自由に行えます。
制限されるのはYorishiro自体を競合するホスティング／マネージドサービスとして提供することのみです。
2030-07-14に自動的にGNU General Public License, Version 2.0以降へ移行します。
