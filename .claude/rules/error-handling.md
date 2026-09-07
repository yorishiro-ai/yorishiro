# Error handling

## YorishiroError

`src/error.rs` defines the error type. All variants are:

| Variant | HTTP status | Machine code |
|---|---|---|
| `ValidationFailed` | 422 | `validation_failed` |
| `NotFound` | 404 | `not_found` |
| `ScopeInsufficient` | 403 | `scope_insufficient` |
| `Conflict` | 409 | `conflict` |
| `RelationTypeMismatch` | 422 | `relation_type_mismatch` |
| `Unauthenticated` | 401 | `unauthenticated` |
| `Maintenance` (read_only) | 423 | `maintenance_read_only` |
| `Maintenance` (full_lock) | 503 | `maintenance_full_lock` |
| `ProviderBusy` | 503 | `provider_busy` |
| `ProviderUnreachable` | 502 | `provider_unreachable` |
| `BackendUnsupported` | 501 | `backend_unsupported` |
| `BackendUnavailable` | 503 | `backend_unavailable` |
| `Internal` | 500 | `internal` |

## Construction

- Use `crate::ResultExt` (`.internal()`) for any fallible call that produces a non-`YorishiroError` error. Never write `map_err(|e| YorishiroError::Internal(e.into()))` by hand.
- `.internal()` only converts an existing error and cannot attach a message. A local helper like `fn internal(message: impl Display)` (see `src/services/embedding/local.rs`) is the sanctioned pattern when a module needs it repeatedly. Do not promote to a shared API until a second module wants it.
- Use `YorishiroError::not_found(msg)` for NotFound construction.
- `code()` returns a machine-readable identifier for every variant. A new variant with no arm in `code()` fails to compile. Do not add a variant without adding its code.

## HTTP mapping

`into_http_parts()` (in `src/error.rs`) maps `YorishiroError` to `(status, JSON body)`. `ApiError` (`src/controllers/error.rs`) calls it. Never duplicate the match block. `ApiError` is currently the only such wrapper: its handlers return `ApiError` like base's do.

**Exception:** `ee/controllers/stripe.rs::stripe_webhook` returns `impl IntoResponse` with raw status codes because Stripe expects simple text, not a JSON envelope. It is the sole exception.

## Conversion to Loco

`YorishiroError` stays the primary error type; it is not being replaced by `loco_rs::Error`. A `From<YorishiroError> for loco_rs::Error` impl (`crate::error`) exists for paths that return `loco_rs::Result` (a `Hooks` method, a task, a worker): use `?` there rather than hand-rolling a `map_err`. `loco_rs::Error::CustomError::ErrorDetail` has no `hint` field, so the conversion folds the whole `into_http_parts()` body into `ErrorDetail::errors` rather than dropping it.
