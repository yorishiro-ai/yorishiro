# Yorishiro (依り代)

**English** | [日本語](README.jp.md)

An MCP-native, multi-tenant knowledge store with user-defined schemas.

Define entity types as JSON meta-schemas (fields, constraints, relations). Data validated against those schemas is accessible through both a REST API and MCP (Model Context Protocol). Fields marked `x-embed` are automatically vector-embedded, enabling similarity search over natural-language queries.

## What it does

- **Schema-driven data.** Define your own entity types with fields, validation rules, and cross-entity relations.
- **MCP integration.** Connect Claude, Cursor, or any MCP-compatible client with 23 built-in tools for searching, creating, and managing entities.
- **REST API.** Programmatic access via standard HTTP endpoints with API key authentication.
- **Vector search.** Natural-language similarity search across embedded text fields.
- **Text search.** Full-text search across all entities, including those without embeddings.
- **Snapshots and restore.** Point-in-time entity recovery via workspace snapshots.
- **Multi-tenancy.** Organize users and workspaces into tenants with role-based access (owner, admin, member, viewer).

## Architecture

```mermaid
flowchart TD
    MCPClient["MCP client<br/>(Claude, etc.)"]
    RESTClient["REST client<br/>(curl/SDK)"]

    subgraph Enterprise["ee/ (enterprise edition)"]
        EnterpriseREST["marketplace / origin / billing / OAuth / LLM keys"]
    end

    subgraph Server["yorishiro (axum)"]
        Core["core<br/>(schemas / entities / search / auth)"]
        MCPAdapter["MCP adapter<br/>(23 tools)"]
        RESTAdapter["REST adapter"]
        MCPAdapter --> Core
        RESTAdapter --> Core
    end

    DB[("PostgreSQL 18 + pgvector<br/>(RLS isolation)")]
    SQLiteDB[("SQLite (single-tenant, sqlite-vec)")]

    MCPClient -->|"/mcp"| MCPAdapter
    RESTClient -->|"/api/*"| RESTAdapter
    RESTAdapter --> Core
    EnterpriseREST --> Core
    Core --> DB
    Core --> SQLiteDB
```

## Editions

| | Without licence | With `YORISHIRO_LICENSE_KEY` |
|---|---|---|
| Core features | Served | Served |
| Enterprise features (marketplace, billing, OAuth, LLM inference) | `404` | Served |
| Web UI | Served either way | |

One repository, one binary. Configuration decides what is enabled. The licence check runs per-request, so an expired key stops enterprise features without a restart.

## Quick start

The fastest path uses Docker. No model downloads or manual setup required.

1. Start the server:

   ```console
   $ docker run -d --name yorishiro --restart unless-stopped -p 8080:8080 \
       -e DATABASE_URL=postgres://user:pass@host:5432/yorishiro \
       ghcr.io/yorishiro-ai/yorishiro:latest
   ```

2. Open `http://localhost:8080/` and create the owner account through the setup wizard.

3. Generate an API key and start creating entities.

For building from source:

```console
$ git clone https://github.com/yorishiro-ai/yorishiro && cd yorishiro
$ make init
```

This starts PostgreSQL and the app via Docker Compose.

## Configuration

Environment variables control behavior at runtime. Common settings:

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | *(required)* | PostgreSQL connection string |
| `YORISHIRO_MAX_TENANTS` | `1` | Tenant cap (`0` = unlimited) |
| `YORISHIRO_LICENSE_KEY` | *(empty)* | Enterprise licence key |
| `YORISHIRO_EMBEDDING_PROVIDER` | *(empty)* | Embedding backend (`local` for local model) |

See [docs/configuration.md](docs/configuration.md) for the full list.

## SQLite mode

Yorishiro can run on SQLite for local evaluation and single-tenant personal use. Vector search works via sqlite-vec. Multi-tenant hosting is not supported (no database-level isolation).

See [docs/sqlite.md](docs/sqlite.md) for details.

## Documentation

| Document | Contents |
|---|---|
| [docs/configuration.md](docs/configuration.md) | All environment variables: embedding, search quotas, logging |
| [docs/sqlite.md](docs/sqlite.md) | SQLite mode: capabilities and limitations |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Code layout, tests, pre-push checks |
| [AGENTS.md](AGENTS.md) | AI agent focus rules |

Not yet written: a meta-schema guide, a REST/MCP API reference, and a deployment guide.

## License

Licensed under the [Business Source License 1.1](LICENSE). Self-hosting is permitted; the only restriction is offering Yorishiro as a competing hosted service. On 2030-07-14 this version automatically converts to the GNU General Public License, Version 2.0 or later.
