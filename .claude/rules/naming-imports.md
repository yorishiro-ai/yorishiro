# Imports and naming

## Imports

- Always `use axum::http::StatusCode;`.
  Never use the fully-qualified `axum::http::StatusCode` inline in function signatures or bodies.
- Group imports: std → external crates → workspace crates → crate-internal.
  `cargo fmt` handles ordering within groups.

## Naming

- The newtype wrapper over `YorishiroError` for axum is `ApiError` (`src/controllers/error.rs`).
  The name is fixed: do not rename.
- Avoid naming collisions across layers.
  A type that wraps or extends another should not reuse its name unless it is a deliberate newtype that disambiguates at every call site (e.g. `controllers::extractors::AuthContext` wraps `auth::AuthContext` as `pub struct AuthContext(pub auth::AuthContext)`, forcing the caller to decide which layer they mean).
