# yorishiro-core

Domain logic for [Yorishiro](https://github.com/yotsunagi/yorishiro) — an MCP-native, multi-tenant knowledge store with user-defined schemas.

This crate contains the core domain layer: metaschema validation/versioning/projection, entity/relation/schema repositories (sea-query + sqlx), authentication/authorization, embedding providers (local ONNX / OpenAI-compatible), and vector search. It is consumed by `yorishiro-server` (the HTTP layer) and `ee/crates/yorishiro-hosted` (the paid features and the binary), and is not meant to be used standalone.

## Features

- **User-defined schemas** using standard JSON Schema keywords (string/number/integer/boolean/array/object with nesting up to 5 levels)
- **Schema versioning** with automatic breaking-change detection
- **Entity CRUD** with JSON Schema validation
- **Typed directed relations** between entities (knowledge graph)
- **Semantic search** via pgvector + pg_trgm fuzzy matching
- **Multi-tenant isolation** via PostgreSQL Row Level Security
- **Embedding providers**: local ONNX (default) or any OpenAI-compatible API

## License

Licensed under the [Business Source License 1.1](https://github.com/yotsunagi/yorishiro/blob/master/LICENSE).
