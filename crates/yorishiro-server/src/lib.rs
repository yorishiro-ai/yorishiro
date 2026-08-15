use std::sync::Arc;

use anyhow::Result;
use yorishiro_core::services::embedding::onnx::{LocalOnnxConfig, LocalOnnxProvider, Pooling};
use yorishiro_core::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

pub mod admin;
pub mod config;
mod error;
pub mod http;
pub mod logging;
mod routes;
mod state;

pub use routes::{
    apply_body_limit_layer, apply_observability_layers, build_app, build_app_with_rate_limiter,
};
pub use state::AppState;

/// `YORISHIRO_MAX_TENANTS` is process-wide state read by both `http::controllers::setup` and login's
/// workspace auto-resolution, so every test across the crate that sets it (rather than just
/// asserting the default) must serialize through this one shared lock -- a per-module lock
/// only prevents that module's own tests from racing each other, not tests in a different
/// module running concurrently in the same `cargo test` process. `#[cfg(test)]`-gated and
/// `pub(crate)`: `tests/` reaches it as `crate::max_tenants_env_lock`, since every test file
/// compiles as its own module's `mod tests` rather than as an external integration test.
#[cfg(test)]
pub(crate) mod max_tenants_env_lock {
    pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub fn set(value: Option<&str>) {
        match value {
            Some(v) => unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", v) },
            None => unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") },
        }
    }
}

/// Shared test-only fixtures used by the crate-root integration tests in `tests/`.
///
/// `#[cfg(test)]`-gated and `pub(crate)`: `tests/` reaches it as `crate::test_support`, since
/// every test file compiles as its own module's `mod tests` rather than as an external
/// integration test. It is therefore never part of a release build or of this crate's public
/// API.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
    use sea_query_binder::SqlxBinder;
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;
    use yorishiro_core::YorishiroError;
    use yorishiro_core::db::TenantDb;
    use yorishiro_core::repositories::tenancy;
    use yorishiro_core::services::auth::create_api_key;
    use yorishiro_core::services::embedding::EmbeddingProvider;

    use crate::AppState;

    #[derive(Iden)]
    enum Tenants {
        Table,
        Id,
        Name,
    }

    #[derive(Iden)]
    enum Workspaces {
        Table,
        Id,
        TenantId,
        Name,
    }

    /// Tests shouldn't call out to a remote embeddings service, so this dummy provider only
    /// satisfies the dimension count (and errors immediately if actually invoked).
    pub struct UnreachableEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for UnreachableEmbeddingProvider {
        fn dimensions(&self) -> usize {
            768
        }

        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Err(YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider should not be called in this test"
            )))
        }
    }

    pub fn test_state(pool: PgPool) -> AppState {
        AppState::new(
            TenantDb::new(pool.clone()),
            pool,
            Arc::new(UnreachableEmbeddingProvider),
        )
    }

    /// The `static_fallback` argument for tests in this crate, which assert about API routes and
    /// never about the SPA. It 404s, matching what this crate serves on its own: the UI lives in
    /// `ee/` and a crate here cannot reach it.
    pub fn no_static_fallback() -> axum::routing::MethodRouter {
        axum::routing::any(|| async { StatusCode::NOT_FOUND })
    }

    /// A provider that returns a deterministic vector, for end-to-end tests of the embedding
    /// wiring. Every text maps to the same vector, so the distance between query and entity
    /// is always 0 — guaranteeing a hit.
    pub struct FixedEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedEmbeddingProvider {
        fn dimensions(&self) -> usize {
            768
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Ok(texts.iter().map(|_| vec![0.1_f32; 768]).collect())
        }
    }

    pub async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Tenants::Table))
            .columns([Tenants::Name])
            .values_panic(["test-tenant".into()])
            .returning(Query::returning().columns([Tenants::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (tenant_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();

        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Workspaces::Table))
            .columns([Workspaces::TenantId, Workspaces::Name])
            .values_panic([tenant_id.into(), "test-workspace".into()])
            .returning(Query::returning().columns([Workspaces::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (workspace_id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();
        (tenant_id, workspace_id)
    }

    /// Extracts the `data: {...}` line from a `text/event-stream` body and parses it as JSON.
    /// streamable-http returns multiple events separated by `\n\n`, but the response to a
    /// single request is carried in the last one, so that's the one targeted.
    pub fn parse_sse_json(body: &str) -> serde_json::Value {
        body.split("\n\n")
            .filter_map(|event| event.lines().find_map(|line| line.strip_prefix("data: ")))
            .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
            .last()
            .unwrap_or_else(|| panic!("no `data:` line found in SSE body: {body:?}"))
    }

    pub async fn mcp_post(
        app: &Router,
        session_id: Option<&str>,
        auth_header: Option<&str>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");

        if let Some(session_id) = session_id {
            builder = builder.header("mcp-session-id", session_id);
        }
        if let Some(auth_header) = auth_header {
            builder = builder.header("authorization", auth_header);
        }

        app.clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    /// Performs the initialize + notifications/initialized handshake and returns the
    /// session ID to use for subsequent tools/call requests.
    pub async fn mcp_handshake(app: &Router) -> String {
        let response = mcp_post(
            app,
            None,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "yorishiro-test", "version": "0.0.0" },
                },
            }),
        )
        .await;
        if response.status() != StatusCode::OK {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            panic!(
                "initialize failed: {status} {}",
                String::from_utf8_lossy(&body)
            );
        }
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .expect("initialize response must carry Mcp-Session-Id")
            .to_str()
            .unwrap()
            .to_string();

        let response = mcp_post(
            app,
            Some(&session_id),
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        session_id
    }

    /// Fills each tool's required arguments with dummy values that only satisfy their types.
    /// The authorization check runs after argument deserialization, so for this test's goal
    /// (catching missing authorization checks) to hold, the arguments themselves must
    /// already be well-formed.
    pub fn dummy_arguments_for_tool(name: &str) -> serde_json::Value {
        const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";
        match name {
            "create_entity" => serde_json::json!({
                "schema_name": "dummy", "entity_type": "dummy", "data": {},
            }),
            "get_entity" => serde_json::json!({ "id": NIL_UUID }),
            "get_entity_drift" => serde_json::json!({ "id": NIL_UUID }),
            "migration_dry_run" => serde_json::json!({ "name": "dummy" }),
            "update_entity" => serde_json::json!({ "id": NIL_UUID, "data": {} }),
            "delete_entity" => serde_json::json!({ "id": NIL_UUID }),
            "list_entities" => serde_json::json!({}),
            "create_relation" => serde_json::json!({
                "source_id": NIL_UUID, "target_id": NIL_UUID, "relation_type": "dummy",
            }),
            "get_relation" => serde_json::json!({ "id": NIL_UUID }),
            "delete_relation" => serde_json::json!({ "id": NIL_UUID }),
            "list_relations" => serde_json::json!({}),
            "set_relation_status" => serde_json::json!({ "id": NIL_UUID, "status": "archived" }),
            "list_schemas" => serde_json::json!({}),
            "get_active_schema" => serde_json::json!({ "name": "dummy" }),
            "get_schema_by_id" => serde_json::json!({ "schema_id": NIL_UUID }),
            "create_schema" => serde_json::json!({ "definition": {} }),
            "get_entity_type_json_schema" => serde_json::json!({
                "schema_name": "dummy", "entity_type": "dummy",
            }),
            "search_entities" => serde_json::json!({ "query_text": "dummy" }),
            "recall_context" => serde_json::json!({ "entity_id": NIL_UUID }),
            "import_jsonl" => serde_json::json!({ "jsonl": "" }),
            "list_templates" => serde_json::json!({}),
            "list_template_library" => serde_json::json!({}),
            "get_template_library_item" => serde_json::json!({ "id": NIL_UUID }),
            other => panic!("no dummy arguments registered for tool `{other}`"),
        }
    }

    pub async fn rest_request(
        app: &Router,
        method: &str,
        uri: &str,
        auth_header: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(auth_header) = auth_header {
            builder = builder.header("authorization", auth_header);
        }
        let body = match body {
            Some(json) => {
                builder = builder.header("content-type", "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };

        app.clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap()
    }

    pub async fn rest_json_body(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    pub async fn seed_task_and_project(
        app: &Router,
        schema_auth: &str,
        write_auth: &str,
    ) -> (String, String) {
        let response = rest_request(
            app,
            "POST",
            "/api/schemas",
            Some(schema_auth),
            Some(serde_json::json!({
                "name": "task-management",
                "entity_types": {
                    "task": { "fields": { "title": { "type": "string", "required": true } } },
                    "project": { "fields": { "name": { "type": "string", "required": true } } }
                },
                "relation_types": {
                    "belongs_to": { "source": "task", "target": "project" }
                },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = rest_request(
            app,
            "POST",
            "/api/entities",
            Some(write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "task",
                "data": { "title": "buy milk" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let task_id = rest_json_body(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let response = rest_request(
            app,
            "POST",
            "/api/entities",
            Some(write_auth),
            Some(serde_json::json!({
                "schema_name": "task-management",
                "entity_type": "project",
                "data": { "name": "groceries" },
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let project_id = rest_json_body(response).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        (task_id, project_id)
    }

    /// Issues an API key attributed to `user_id`, scoped to `role`'s max scope -- exactly what
    /// `/auth/login` would hand out for that role.
    pub async fn issue_key_for(
        pool: &PgPool,
        tenant_id: Uuid,
        workspace_id: Uuid,
        user_id: Uuid,
        role: tenancy::MembershipRole,
    ) -> String {
        let db = TenantDb::new(pool.clone());
        let mut conn = db
            .acquire_for_workspace(tenant_id, workspace_id)
            .await
            .unwrap();
        create_api_key(&mut conn, workspace_id, role.max_scope(), Some(user_id))
            .await
            .unwrap()
            .plaintext
    }
}

/// Starts a graceful shutdown on Ctrl-C (all platforms) or SIGTERM (Unix only,
/// the standard stop signal from container orchestrators).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install ctrl-c handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining connections");
}

/// The exit code a binary uses when it stops because its configuration is absent or unusable,
/// rather than because something it depends on is not ready yet.
///
/// `EX_CONFIG` from `sysexits.h`. The units set `RestartPreventExitStatus=78`, which is the
/// whole point of having a distinct code: an unconfigured start is not a fault that waiting
/// fixes, so systemd must stop rather than retry every five seconds forever. Anything else
/// still exits 1 and keeps its retry -- a database that has not finished starting is exactly
/// the case `Restart=on-failure` exists for.
///
/// Measured before this existed: an unconfigured `enable --now` restarted 15 times in 45
/// seconds, and `systemctl is-failed` answered `activating` throughout, so nothing watching
/// unit state could ever see it.
pub const EXIT_CONFIG: i32 = 78;

/// `DATABASE_URL`, or an error an operator can act on.
///
/// The obvious `.expect("DATABASE_URL must be set")` prints a Rust panic naming a source file,
/// which is the wrong audience: most people meet this message after installing a package, having
/// never seen the source. This names the file the package actually puts the setting in.
///
/// Callers that are a `main` should report this through [`exit_with_config_code`] so the
/// process carries [`EXIT_CONFIG`] rather than a bare 1.
pub fn database_url_from_env() -> Result<String> {
    non_empty_env("DATABASE_URL").ok_or_else(|| {
        anyhow::anyhow!(
            "DATABASE_URL is not set.\n\n\
             Set it in /etc/yorishiro/yorishiro.env (package installs), in the environment, or \n\
             as `database_url:` in config.yml, then start the server again. For example:\n\n    \
             DATABASE_URL=postgres://yorishiro:PASSWORD@localhost:5432/yorishiro\n\n\
             The database must exist and the role must be able to create the schema; the server \n\
             applies its migrations on startup."
        )
    })
}

/// Prints `error` the way `fn main() -> Result<()>` would and exits with [`EXIT_CONFIG`].
///
/// Returning the `Err` from `main` prints the same text but always exits 1, which is
/// indistinguishable from a database that is not up yet -- and the two want opposite handling
/// from systemd. This keeps the message identical and changes only the code.
///
/// Returns `T` rather than `!` only so it can be passed straight to `unwrap_or_else`, which
/// needs the closure's return type to match the `Ok` type; it never actually returns.
pub fn exit_with_config_code<T>(error: anyhow::Error) -> T {
    eprintln!("Error: {error:?}");
    std::process::exit(EXIT_CONFIG)
}

/// Reads an environment variable, treating both "unset" and "set to an empty string" as absent.
/// `env::var(...).ok()` alone would treat `FOO=` (set but empty) as present, which for a bind
/// address means handing `TcpListener::bind` an empty string and aborting startup.
pub fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// The default bind address when `YORISHIRO_BIND` is unset or empty.
pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8081";

/// `YORISHIRO_BIND`, defaulting to [`DEFAULT_BIND_ADDR`].
///
/// Here rather than in `ee/` because both binaries need it and neither edition owns the idea of
/// a listen address. `ee/` re-exports it.
pub fn bind_addr_from_env() -> String {
    bind_addr_or_default(non_empty_env("YORISHIRO_BIND").as_deref())
}

/// The pure fold [`bind_addr_from_env`] wraps, split out so it is testable without touching the
/// process environment: `None` and `Some("")` both fall back to [`DEFAULT_BIND_ADDR`].
pub fn bind_addr_or_default(raw: Option<&str>) -> String {
    raw.filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_BIND_ADDR)
        .to_string()
}

/// The model name this deployment is configured for, for stamping onto new workspaces.
///
/// Read from the environment rather than the provider: `EmbeddingProvider` exposes
/// `dimensions()` because callers need it, and adding a `model()` for one caller would put a
/// naming question ("what does a local ONNX file call itself?") into a trait every downstream
/// implementation would have to answer.
pub fn embedding_model_name() -> String {
    match std::env::var("YORISHIRO_EMBEDDING_PROVIDER")
        .unwrap_or_else(|_| "local".into())
        .as_str()
    {
        "openai" => std::env::var("YORISHIRO_EMBEDDING_MODEL").unwrap_or_else(|_| "openai".into()),
        _ => std::env::var("YORISHIRO_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "multilingual-e5-large".into()),
    }
}

/// Builds the embeddings provider from environment variables. `YORISHIRO_EMBEDDING_PROVIDER`
/// switches between `local` (a local ONNX model, the default -- needs no external service or
/// API key, just the model files under `models/`) and `openai` (an OpenAI-compatible API, for
/// operators already running something like Ollama/LM Studio). The `entities.embedding`
/// column is `vector` (dimensionless), so any model works; all vectors in a deployment must
/// share the same dimension count (set via `YORISHIRO_EMBEDDING_DIMENSIONS`, default 1024 — the
/// width of the default model, multilingual-e5-large).
pub fn build_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "1024".into())
        .parse()?;

    let kind = std::env::var("YORISHIRO_EMBEDDING_PROVIDER").unwrap_or_else(|_| "local".into());
    match kind.as_str() {
        "openai" => {
            let base_url = std::env::var("YORISHIRO_EMBEDDING_BASE_URL").map_err(|_| {
                anyhow::anyhow!(
                    "YORISHIRO_EMBEDDING_BASE_URL must be set when YORISHIRO_EMBEDDING_PROVIDER=openai"
                )
            })?;
            let model = std::env::var("YORISHIRO_EMBEDDING_MODEL").map_err(|_| {
                anyhow::anyhow!(
                    "YORISHIRO_EMBEDDING_MODEL must be set when YORISHIRO_EMBEDDING_PROVIDER=openai"
                )
            })?;
            let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
                base_url: base_url.clone(),
                api_key: std::env::var("YORISHIRO_EMBEDDING_API_KEY").unwrap_or_default(),
                model: model.clone(),
                dimensions,
                send_dimensions_param: std::env::var("YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM")
                    .map(|v| v == "true")
                    .unwrap_or(true),
            });
            tracing::info!(provider = "openai", %base_url, %model, dimensions, "embedding provider configured");
            Ok(Arc::new(provider))
        }
        "local" => {
            let max_sequence_length: usize = std::env::var("YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH")
                .unwrap_or_else(|_| "512".into())
                .parse()?;
            let model_path = std::env::var("YORISHIRO_ONNX_MODEL_PATH")
                .unwrap_or_else(|_| "models/model.onnx".into());
            let tokenizer_path = std::env::var("YORISHIRO_ONNX_TOKENIZER_PATH")
                .unwrap_or_else(|_| "models/tokenizer.json".into());
            // Rejected rather than defaulted on an unknown value: reading a model with the
            // wrong pooling does not fail, it just returns worse vectors.
            let pooling = match std::env::var("YORISHIRO_ONNX_POOLING") {
                Ok(value) => Pooling::parse(&value)?,
                Err(_) => Pooling::default(),
            };
            // Empty is treated as unset: an operator clearing the variable means "no prefix",
            // not "prefix with nothing".
            let query_instruction = std::env::var("YORISHIRO_ONNX_QUERY_INSTRUCTION")
                .ok()
                .filter(|value| !value.trim().is_empty());
            let provider = LocalOnnxProvider::load(LocalOnnxConfig {
                model_path: model_path.clone().into(),
                tokenizer_path: tokenizer_path.clone().into(),
                dimensions,
                max_sequence_length,
                pooling,
                query_instruction,
            })
            .map_err(|err| {
                anyhow::anyhow!(
                    "{err}\n\nThe local ONNX embedding provider (the default; see \
                     YORISHIRO_EMBEDDING_PROVIDER) needs '{model_path}' and '{tokenizer_path}' -- \
                     these are not bundled in the repository or the Docker image, and must be \
                     fetched separately. See docs/setup.md#prerequisites for the download \
                     commands, or set YORISHIRO_EMBEDDING_PROVIDER=openai to use an OpenAI-compatible \
                     endpoint instead (see docs/embedding-providers.md)."
                )
            })?;
            tracing::info!(provider = "local", %model_path, dimensions, ?pooling, "embedding provider configured");
            Ok(Arc::new(provider))
        }
        other => {
            anyhow::bail!(
                "unknown YORISHIRO_EMBEDDING_PROVIDER '{other}' (expected 'openai' or 'local')"
            )
        }
    }
}
