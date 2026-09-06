# Yorishiro (依り代)

**English** | [日本語](README.jp.md)

A knowledge store where you define your own data structures and search by meaning, not just keywords.

## What it does

- Define your own data structures
  - Create entity types with fields, validation rules, and cross-entity relations. No code changes needed when your data model evolves.
- Search by meaning
  - Type a description like "customer complaint about shipping" and find related records even if none of them contain those exact words.
- Full-text search
  - Fall back to keyword search when embeddings are not yet available.
- Connect to AI tools
  - Claude, Cursor, and other MCP-compatible clients can use 23 built-in tools to search, create, and manage your data.
- REST API
  - Automate everything with standard HTTP endpoints.
- Version recovery
  - Snapshots let you restore individual records to a previous state.
- Organize with teams
  - Tenants and workspaces with role-based access keep data scoped and shared as you choose.

## Architecture

```mermaid
flowchart TD
    MCPClient["MCP client<br/>(Claude, Cursor, etc.)"]
    RESTClient["REST client<br/>(curl, SDK, browser)"]

    subgraph Server["Yorishiro (axum)"]
        MCP["MCP<br/>(23 tools)"]
        REST["REST API"]
        Core["Core + ee/<br/>(schemas / entities / search / auth / marketplace / billing / OAuth / LLM keys)<br/>license-gated routes are 404 when unlicensed"]
    end

    DB[("PostgreSQL<br/>or SQLite")]

    MCPClient -->|"MCP tools"| MCP
    RESTClient -->|"HTTP API"| REST
    MCP --> Core
    REST --> Core
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

1. Open `http://localhost/` and create an account through the setup wizard.
2. Generate an API key and start creating entities.

For installation instructions, see [docs/installation.md](docs/installation.md).

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
| [docs/installation.md](docs/installation.md) | Docker, Docker Compose, deb/rpm, and build from source |
| [docs/configuration.md](docs/configuration.md) | All settings: embedding, search quotas, logging, queue backends |
| [docs/sqlite.md](docs/sqlite.md) | SQLite mode: capabilities and limitations |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Contributing guidelines |
| [AGENTS.md](AGENTS.md) | Notes for AI agents |

## License

Licensed under the [Business Source License 1.1](LICENSE). Self-hosting is permitted. On 2030-07-14 this version converts to the GNU General Public License, Version 2.0 or later.
