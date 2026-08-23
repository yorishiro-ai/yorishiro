# Imports and naming

## Imports

- Always `use axum::http::StatusCode;`.
  Never use the fully-qualified `axum::http::StatusCode` inline in function signatures or bodies.
- Group imports: std → external crates → workspace crates → crate-internal.
  `cargo fmt` handles ordering within groups.

## Naming

- The newtype wrapper over `YorishiroError` for axum is `ApiError` (`yorishiro-server`).
  The name is fixed: do not rename.
- Avoid naming collisions across layers.
  If a type name already exists in `yorishiro-core`, the server-layer type that wraps/extends it should have a distinct name (e.g. core's `AuthContext` vs. server's auth extractor).
