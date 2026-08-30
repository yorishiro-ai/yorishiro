# Error handling

- Use `crate::ResultExt` (`.internal()`) for any fallible call that produces a non-`YorishiroError` error.
  Never write `map_err(|e| YorishiroError::Internal(e.into()))` by hand.
  `.internal()` only converts an existing error (`E: Into<anyhow::Error>`) and cannot attach a message, so it does not cover raising an `Internal` from a formatted string with no source error.
  `src/services/embedding/local.rs` has a private `fn internal(message: impl Display)` for exactly that case: a local helper like it is the sanctioned pattern when a module needs it repeatedly.
  Do not promote one to a shared API until a second module actually wants it.
- Use `YorishiroError::not_found(msg)` for NotFound construction instead of building the struct literal directly.
- The `into_response` mapping from `YorishiroError` to HTTP status+body lives in `YorishiroError::into_http_parts()` (in `crate::error`).
  `ApiError` calls it, and so must any other axum error wrapper built on `YorishiroError`.
  Never duplicate the match block.
  `ApiError` is currently the only such wrapper, and `ee/` has none of its own: its handlers return `ApiError` like base's do.
  That name is fixed; do not rename it.
- The Stripe webhook (`stripe_webhook`) returns a plain `impl IntoResponse` with raw status codes, because Stripe expects simple text rather than a JSON error envelope.
  It is the sole exception to using `ApiError`, which every other handler under `ee/` returns.
- Every `YorishiroError` variant has a machine-readable `code()` (`crate::error`), emitted as `error.code` in the JSON body by `into_http_parts()`.
  A new variant with no arm in `code()`'s match fails to compile, which is what makes `ValidationFailed` and `RelationTypeMismatch` distinguishable on the wire even though both answer 422.
  Do not add a variant without adding its code.
- `YorishiroError` stays the primary error type; it is not being replaced by `loco_rs::Error`.
  A `From<YorishiroError> for loco_rs::Error` impl (`crate::error`) exists for the few paths that must return `loco_rs::Result` instead of an axum handler's `Result<_, ApiError>` (a `Hooks` method, a task, a worker): use `?` there rather than hand-rolling a `map_err`.
  `loco_rs::Error::CustomError`'s `ErrorDetail` has no `hint` field, so the conversion folds the whole `into_http_parts()` body into `ErrorDetail::errors` rather than dropping it.
