# Docker Compose

以下を `compose.yml` にコピーし、そのディレクトリで実行します。

## デフォルト（SQLite）

```yaml
services:
  app:
    image: ghcr.io/yorishiro-ai/yorishiro:latest
    restart: unless-stopped
    ports:
      - "80:5150"
    volumes:
      - data:/home/yorishiro/.cache/yorishiro
    environment:
      - DATABASE_URL=sqlite:///var/lib/yorishiro/yorishiro.sqlite3?mode=rwc

volumes:
  data:
```

`docker compose up -d` で開始、`docker compose down` で停止します。

## PostgreSQL

`DATABASE_URL` を書き換えます：

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
    image: valkey/valkey:8
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
    restart: unless-stopped
    environment:
      POSTGRES_USER: user
      POSTGRES_PASSWORD: pass
      POSTGRES_DB: yorishiro
    volumes:
      - pg-data:/var/lib/postgresql/data

  valkey:
    image: valkey/valkey:8
    restart: unless-stopped
    volumes:
      - valkey-data:/data

volumes:
  pg-data:
  valkey-data:
```

全設定は [docs/ja/configuration.md](../configuration.md) を参照してください。
