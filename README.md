# Yorishiro (依り代)

**English** | [日本語](README.jp.md)

[Install](#get-started) · [Quick start](https://yorishiro-ai.github.io/wiki/installation/first-run/) · [Documentation](https://yorishiro-ai.github.io/wiki/) · [Releases](https://github.com/yorishiro-ai/yorishiro/releases)

**A knowledge store with schemas you define, read and written by AI agents.**

Yorishiro stores structured data and the connections between it.
Through MCP, you can create schemas, write data, and search by meaning for the information you need.

- **Choose the structure you store.** Define entities with your own schemas and manage data against them.
- **Find more than matching words.** Search entities by meaning, not just keywords.
- **Read and write through MCP.** Schema creation, entity retrieval and updates, and search are available as MCP tools. [Tool reference](https://yorishiro-ai.github.io/wiki/reference/mcp-tools/)
- **Keep the connections, too.** Create relations between entities, then list or retrieve those stored relations individually.
- **Start with templates.** Use templates to set up your schemas. [Template guide](https://yorishiro-ai.github.io/wiki/guides/templates/)
- **Run it in your own environment.** SQLite supports a single tenant; PostgreSQL supports RLS-based tenant isolation. [SQLite support](https://yorishiro-ai.github.io/wiki/guides/sqlite/)

---

## Get started

Run the server, worker, and scheduler with Docker Compose.
Save a configuration from the [Docker Compose guide](https://yorishiro-ai.github.io/wiki/installation/docker-compose/) as `compose.yml` and replace the image's `<VERSION>` with the release tag you want to use.
Run this in the directory containing that file:

```bash
docker compose up -d
```

Connect to the running server at `http://localhost:5150`.
Follow the [first-run guide](https://yorishiro-ai.github.io/wiki/installation/first-run/) to create your first account and API key.

See the [Docker Compose guide](https://yorishiro-ai.github.io/wiki/installation/docker-compose/) for data storage, embedding models, and database options.
For installation without Docker, see [packages](https://yorishiro-ai.github.io/wiki/installation/packages/) or [building from source](https://yorishiro-ai.github.io/wiki/installation/build/).

## Documentation

Usage, configuration, and operations documentation lives in the [wiki](https://yorishiro-ai.github.io/wiki/).
This README covers the overview and getting started; detailed documentation is maintained in the wiki.

[Installation](https://yorishiro-ai.github.io/wiki/installation/) · [Configuration](https://yorishiro-ai.github.io/wiki/configuration/) · [Embeddings](https://yorishiro-ai.github.io/wiki/configuration/embedding/) · [Workers and queues](https://yorishiro-ai.github.io/wiki/configuration/workers-and-queues/) · [MCP tools](https://yorishiro-ai.github.io/wiki/reference/mcp-tools/)

## Agent instructions

AI agents working on this repository should read [AGENTS.md](AGENTS.md) first.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the code layout, development environment, tests, and checks before submitting changes.
Local dependency fixes are documented in [patches/README.md](patches/README.md).

## License

Licensed under the [Business Source License 1.1](LICENSE).
Self-hosting is permitted.
On 2030-07-14 this version converts to the GNU General Public License, Version 2.0 or later.
