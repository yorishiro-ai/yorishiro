# Docker Compose

Copy the following to `compose.yml` and run from that directory.
The `80:5150` mapping exposes the server on host port `80`; the server listens on port `5150` in the container.

## Default (SQLite)

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

Start with `docker compose up -d`. Stop with `docker compose down`.

The local embedding provider downloads its model on first use.
Set `YORISHIRO_EMBEDDING_PROVIDER=none` to skip that download when embedding is not required.

## PostgreSQL

In the default example, replace the `DATABASE_URL` line:

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

See [configuration](../configuration.md) for all settings.
