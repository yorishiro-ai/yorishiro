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

    subgraph Enterprise["ee/ (enterprise edition)"]
        EnterpriseREST["marketplace / billing / OAuth / LLM keys"]
    end

    subgraph Server["Yorishiro (axum)"]
        Core["Core<br/>(schemas / entities / search / auth)"]
        MCP["MCP<br/>(23 tools)"]
        REST["REST API"]
    end

    DB[("PostgreSQL<br/>or SQLite")]

    MCPClient -->|"MCP tools"| MCP
    RESTClient -->|"HTTP API"| REST
    REST --> Core
    EnterpriseREST --> Core
    Core --> DB
```

## Editions

| | Free | Enterprise |
|---|---|---|
| Core features | Available | Available |
| Enterprise features (marketplace, billing, OAuth, LLM inference) | Not available | Available |

One binary, one repository. Enterprise features are controlled by a licence key.

## Quick start

The easiest way is Docker:

```console
$ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
    -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
    ghcr.io/yorishiro-ai/yorishiro:latest
```

1. Open `http://localhost:8080/` and create an account through the setup wizard.
2. Generate an API key and start creating entities.

To build from source:

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
```

## Configuration

Most settings come from environment variables:

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | *(required)* | Database connection |
| `YORISHIRO_MAX_TENANTS` | `1` | Tenant limit (`0` = unlimited) |
| `YORISHIRO_LICENSE_KEY` | *(empty)* | Enterprise licence key |
| `YORISHIRO_EMBEDDING_PROVIDER` | *(empty)* | Embedding backend (`local` for local model) |

See [docs/configuration.md](docs/configuration.md) for the full list.

## SQLite mode

You can run Yorishiro on SQLite for local use and personal projects. Vector search works via sqlite-vec. Multi-tenant hosting is not supported on SQLite.

See [docs/sqlite.md](docs/sqlite.md) for details.

## Documentation

| Document | Contents |
|---|---|
| [docs/configuration.md](docs/configuration.md) | All settings: embedding, search quotas, logging |
| [docs/sqlite.md](docs/sqlite.md) | SQLite mode: capabilities and limitations |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Contributing guidelines |
| [AGENTS.md](AGENTS.md) | Notes for AI agents |

## License

Licensed under the [Business Source License 1.1](LICENSE). Self-hosting is permitted. On 2030-07-14 this version converts to the GNU General Public License, Version 2.0 or later.
