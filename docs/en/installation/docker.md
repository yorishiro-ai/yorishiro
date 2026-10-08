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
    -v yorishiro-data:/var/lib/yorishiro \
    -v yorishiro-model-cache:/home/yorishiro/.cache/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

## Configuration

See [docs/en/configuration.md](../configuration.md) for all settings.
The image contains the reference-only configuration at `/app/config/example.yaml`.
When `/app/config/production.yaml` and `/app/config/production.local.yaml` are absent, the binary uses the embedded bytes of `config/example.yaml`.
To configure the container with files, mount a complete administrator-owned `/app/config/production.yaml` and optionally `/app/config/production.local.yaml`, or replace `LOCO_CONFIG_FOLDER` with a directory containing those files.
The image never creates, merges, rewrites, chmods, or removes mounted configuration files.
The reference default binds to `127.0.0.1`; Docker Compose sets `BINDING=0.0.0.0` explicitly because it publishes port `5150`.
