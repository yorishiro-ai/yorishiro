# Yorishiro (依り代)

**English** | [日本語](README.jp.md)

[![codecov](https://codecov.io/gh/yorishiro-ai/yorishiro/graph/badge.svg)](https://codecov.io/gh/yorishiro-ai/yorishiro) [![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/yorishiro-ai/yorishiro)

[Install](#get-started) · [Quick start](https://yorishiro-ai.github.io/wiki/installation/first-run/) · [Documentation](https://yorishiro-ai.github.io/wiki/) · [Releases](https://github.com/yorishiro-ai/yorishiro/releases)

**A knowledge store for AI agents, with schemas you define.**

Use [MCP tools](https://yorishiro-ai.github.io/wiki/reference/mcp-tools/) to define schemas, read and write entities, manage relations, and search by meaning.
Use [templates](https://yorishiro-ai.github.io/wiki/guides/templates/) as a starting point for schemas.
SQLite supports a [single tenant](https://yorishiro-ai.github.io/wiki/guides/sqlite/); PostgreSQL isolates tenants with row-level security.

---

## Get started

Run the server, embedding worker, application-jobs worker, and scheduler with Docker Compose.
Save a configuration from the [Docker Compose guide](https://yorishiro-ai.github.io/wiki/installation/docker-compose/) as `compose.yml` and replace the image's `<VERSION>` with the release tag you want to use.
Run this in the directory containing that file:

```bash
docker compose up -d
```

Connect to the running server at `http://localhost:5150`.
Follow the [first-run guide](https://yorishiro-ai.github.io/wiki/installation/first-run/) to create your first account and API key.

For installation without Docker, see [packages](https://yorishiro-ai.github.io/wiki/installation/packages/) or [building from source](https://yorishiro-ai.github.io/wiki/installation/build/).

## Documentation

[Installation](https://yorishiro-ai.github.io/wiki/installation/) · [Configuration](https://yorishiro-ai.github.io/wiki/configuration/) · [Embeddings](https://yorishiro-ai.github.io/wiki/configuration/embedding/) · [Workers and queues](https://yorishiro-ai.github.io/wiki/configuration/workers-and-queues/) · [MCP tools](https://yorishiro-ai.github.io/wiki/reference/mcp-tools/)

The `PowerUserAccess` managed policy excludes IAM and tenant administration and includes read access to the API activity trail, where IAM changes appear as the resources of the request that made them.
Use `WorkspaceViewerAccess` or `WorkspaceContributorAccess` for workspace data access without tenant audit access.

A new PostgreSQL database needs the `vector` and `pg_trgm` extensions installed, and its migration role needs the privilege to create the `yorishiro_app` role, before `yorishiro db migrate` runs; drop that privilege from the bootstrap role afterwards.

Every REST request, MCP call, job attempt, scheduler task and CLI command leaves a row in the API trail: when, from where, by whom, on what and with what outcome.
A row holds identifiers and paging numbers only, never entry data, query text, definitions or keys.
`settings.limits.audit_retention_days` (365 by default) sets how long rows are kept.
Read a tenant's rows with `GET /api/api-events` (filter by `request_id`, `correlation_id` or `resource`); read the rows of a retired tenant, or of no tenant, with `cargo loco task read_audit_events tenant_id=<uuid|none>`.

Yorishiro sends the embedded text of each entity to the provider whole and does not truncate it.
An input above the provider's token limit fails that entity, and a reindex publishes only when every entity succeeds.

## Agent instructions

AI agents working on this repository should read [AGENTS.md](AGENTS.md) first.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the code layout, development environment, tests, and checks before submitting changes.
See [dependency patches](patches/README.md) for local fixes.

## License

Licensed under the [Business Source License 1.1](LICENSE).
Self-hosting is permitted.
On 2030-07-14 this version converts to the GNU General Public License, Version 2.0 or later.
