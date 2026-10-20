# Docker Compose

以下を `compose.yml` にコピーし、そのディレクトリで実行します。
`80:5150` は、ホストのポート `80` をコンテナ内のサーバーポート `5150` に接続します。

## デフォルト（SQLite）

```yaml
services:
  app:
    image: ghcr.io/yorishiro-ai/yorishiro:latest
    restart: unless-stopped
    ports:
      - "80:5150"
    volumes:
      - data:/var/lib/yorishiro
      - model-cache:/home/yorishiro/.cache/yorishiro
    environment:
      - DATABASE_URL=sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc
      - QUEUE_URL=sqlite:///var/lib/yorishiro/yorishiro_queue.sqlite3?mode=rwc

volumes:
  data:
  model-cache:
```

`docker compose up -d` で起動し、`docker compose down` で停止します。

ローカルの埋め込みプロバイダーは初回使用時にモデルをダウンロードします。
埋め込みが不要な場合は `YORISHIRO_EMBEDDING_PROVIDER=none` を設定するとダウンロードを省略できます。

## PostgreSQL

既定の例で `DATABASE_URL` の行を次のように変更します。

```yaml
environment:
  - DATABASE_URL=postgres://user:pass@host:5432/yorishiro
```

## Valkey (Redis)

```yaml
services:
  app:
    image: ghcr.io/yorishiro-ai/yorishiro:latest
    restart: unless-stopped
    ports:
      - "80:5150"
    depends_on:
      - valkey
    environment:
      - YORISHIRO_QUEUE_KIND=Redis
      - QUEUE_URL=redis://valkey:6379

  valkey:
    image: valkey/valkey:9.1
    restart: unless-stopped
    volumes:
      - valkey-data:/data

volumes:
  valkey-data:
```

## PostgreSQL + Valkey

```yaml
services:
  app:
    image: ghcr.io/yorishiro-ai/yorishiro:latest
    restart: unless-stopped
    ports:
      - "80:5150"
    depends_on:
      - postgres
      - valkey
    environment:
      - DATABASE_URL=postgres://user:pass@host:5432/yorishiro
      - YORISHIRO_QUEUE_KIND=Redis
      - QUEUE_URL=redis://valkey:6379

  postgres:
    image: pgvector/pgvector:pg18
    init: true
    restart: unless-stopped
    environment:
      POSTGRES_USER: user
      POSTGRES_PASSWORD: pass
      POSTGRES_DB: yorishiro
    volumes:
      - pg-data:/var/lib/postgresql/data

  valkey:
    image: valkey/valkey:9.1
    restart: unless-stopped
    volumes:
      - valkey-data:/data

volumes:
  pg-data:
  valkey-data:
```

設定項目は [設定](../configuration.md) を参照してください。

PostgreSQLコンテナには、リポジトリの`db`・`testdb`と同じく`init: true`を設定してください。
initプロセスが孤児プロセスを回収することで、無関係な子プロセスの異常終了によるPostgreSQLクラスタのリカバリーを防ぎます。
既存環境へ反映するには、計画したタイミングでコンテナを再作成してください。
Composeファイルを変更しただけでは、稼働中のコンテナには反映されません。
