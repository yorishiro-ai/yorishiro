# Docker Compose

Copy the following to `compose.yml` and run from that directory.

## Default (SQLite)

```yaml
x-yorishiro: &yorishiro
  image: ghcr.io/yorishiro-ai/yorishiro:latest
  restart: unless-stopped
  volumes:
    - state:/var/lib/yorishiro/state
    - model-cache:/home/yorishiro/.cache/yorishiro
  environment:
    - DATABASE_URL=sqlite:///var/lib/yorishiro/state/yorishiro.sqlite3?mode=rwc
    - QUEUE_URL=sqlite:///var/lib/yorishiro/state/yorishiro_queue.sqlite3?mode=rwc

services:
  app:
    <<: *yorishiro
    ports:
      - "80:5150"
  worker:
    <<: *yorishiro
    entrypoint: ["/var/lib/yorishiro/worker-wrapper.sh"]
    command: []
    depends_on:
      - app
  scheduler:
    <<: *yorishiro
    command: ["scheduler"]
    healthcheck:
      disable: true
    depends_on:
      - app

volumes:
  state:
  model-cache:
```

Start with `docker compose up -d`. Stop with `docker compose down`.

Keep the `worker` and `scheduler` services in PostgreSQL and Valkey deployments as well.
Pass the same database and queue settings to all three services.

The local embedding provider downloads about 522 MiB on first use.
Set `YORISHIRO_EMBEDDING_PROVIDER=none` to skip that download when embedding is not required.

## PostgreSQL

Replace the `DATABASE_URL` line:

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

See [docs/configuration.md](../configuration.md) for all settings.
