# Yorishiro (依り代)

**English** | [日本語](README.jp.md)

A knowledge store where you define your own data structures and search by meaning, not just keywords.

## What it does

- **Define your own data structures.** Create entity types with fields, validation rules, and cross-entity relations. No code changes needed when your data model evolves.
- **Search by meaning.** Type a description like "customer complaint about shipping" and find related records even if none of them contain those exact words.
- **Full-text search.** Fall back to keyword search when embeddings are not yet available.
- **Connect to AI tools.** Claude, Cursor, and other MCP-compatible clients can use 23 built-in tools to search, create, and manage your data.
- **REST API.** Automate everything with standard HTTP endpoints.
- **Version recovery.** Snapshots let you restore individual records to a previous state.
- **Organize with teams.** Tenants and workspaces with role-based access keep data scoped and shared as you choose.

## Architecture

```mermaid
flowchart TD
    MCPClient["MCP client<br/>(Claude, Cursor, etc.)"]
    RESTClient["REST client<br/>(curl, SDK, browser)"]

    subgraph Server["Yorishiro (axum)"]
        MCP["MCP<br/>(23 tools)"]
        REST["REST API"]

        subgraph Enterprise["ee/ (enterprise edition)"]
            EE["marketplace / billing / OAuth / LLM keys"]
        end

        Core["Core<br/>(schemas / entities / search / auth)"]
    end

    DB[("PostgreSQL<br/>or SQLite")]

    MCPClient -->|"MCP tools"| MCP
    RESTClient -->|"HTTP API"| REST
    MCP --> Core
    REST --> Core
    EE --> REST
    Core --> DB
```

## Editions

| | Free | Enterprise |
|---|---|---|
| Core features | Available | Available |
| Enterprise features (marketplace, billing, OAuth, LLM inference) | Not available | Available |

One binary, one repository. Enterprise features are controlled by a licence key.

## Quick start

The default configuration uses SQLite — no external database required.

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. Open `http://localhost:8080/` and create an account through the setup wizard.
2. Generate an API key and start creating entities.

### Switching to PostgreSQL

For multi-tenant hosting or vector search, use PostgreSQL:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

### Switching to Valkey (Redis)

For distributed queue processing:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e YORISHIRO_QUEUE_KIND=Redis \
    -e QUEUE_URL=redis://user:pass@host:6379 \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

To build from source:

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
```

## Configuration

Most settings come from environment variables:

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:///var/lib/yotsunagi/yorishiro.sqlite3?mode=rwc` | `postgres://` URI for multi-tenant or vector search |
| `YORISHIRO_MAX_TENANTS` | `1` | Tenant limit (`0` = unlimited) |
| `YORISHIRO_LICENSE_KEY` | *(empty)* | Enterprise licence key |
| `YORISHIRO_EMBEDDING_PROVIDER` | `local` | Embedding backend (`none` disables embeddings) |
| `YORISHIRO_QUEUE_KIND` | Auto-detected | Set to `Redis` for Valkey/Redis queue |
| `QUEUE_URL` | Shares `DATABASE_URL` | Required for Redis queue |

See [docs/configuration.md](docs/configuration.md) for the full list.

## Documentation

| Document | Contents |
|---|---|
| [docs/configuration.md](docs/configuration.md) | All settings: embedding, search quotas, logging, queue backends |
| [docs/sqlite.md](docs/sqlite.md) | SQLite mode: capabilities and limitations |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Contributing guidelines |
| [AGENTS.md](AGENTS.md) | Notes for AI agents |

## License

Licensed under the [Business Source License 1.1](LICENSE). Self-hosting is permitted. On 2030-07-14 this version converts to the GNU General Public License, Version 2.0 or later.
