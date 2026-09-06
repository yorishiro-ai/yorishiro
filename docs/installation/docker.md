# Docker Installation

Yorishiro ships as a single binary. Docker is the simplest option.

## Quick start

The default configuration uses SQLite — no external database required.

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. Open `http://localhost/` and create an account through the setup wizard.
2. Generate an API key and start creating entities.

## Switching to PostgreSQL

For multi-tenant hosting or vector search:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Switching to Valkey (Redis)

For distributed queue processing:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -e YORISHIRO_QUEUE_KIND=Redis \
    -e QUEUE_URL=redis://user:pass@host:6379 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Volume persistence

By default Docker stores data inside the container. To persist data:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 80:5150 \
    -v yorishiro-data:/home/yorishiro/.cache/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Configuration

See [docs/configuration.md](configuration.md) for all settings.
The production config file is at `/app/config/production.yaml` inside the image.
