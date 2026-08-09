use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

/// Upper bound for the DB connectivity probe. Kept well below the orchestrator's (e.g.
/// k8s) health check timeout (typically a few seconds) so that, even if the database is
/// unresponsive, `/health` itself returns 503 before it would hang long enough to trip the
/// orchestrator's own timeout.
const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

/// The `/up` handler: a liveness probe that only confirms the process is running and able
/// to answer HTTP requests. Unlike `/health`, it never touches the database, so it stays
/// fast and healthy even during a DB outage — an orchestrator should use this to decide
/// whether to restart the process, and `/health` to decide whether to route traffic to it.
pub async fn up_check() -> (StatusCode, Json<HealthResponse>) {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

/// The `/health` handler. Simply always returning `{"status":"ok"}` would keep telling the
/// orchestrator the instance is healthy even during a DB outage or pool exhaustion, making
/// it impossible to detect and evict a broken instance. So this actually probes the
/// database with a lightweight query and returns 503 (Service Unavailable) on failure.
///
/// This check doesn't need an RLS tenant context, so it just grabs a connection directly
/// from the pool instead of going through `TenantDb::acquire_for_workspace` (which also sets
/// `app.current_tenant`).
pub async fn health_check(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match check_db(&state).await {
        Ok(()) => (StatusCode::OK, Json(HealthResponse { status: "ok" })),
        Err(err) => {
            tracing::warn!(error = %err, "health check: database unreachable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "unavailable",
                }),
            )
        }
    }
}

async fn check_db(state: &AppState) -> Result<(), sqlx::Error> {
    let pool = state.tenant_db.pool().clone();
    let probe = async move {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT 1").execute(conn.as_mut()).await?;
        Ok::<(), sqlx::Error>(())
    };

    match tokio::time::timeout(DB_CHECK_TIMEOUT, probe).await {
        Ok(result) => result,
        Err(_) => Err(sqlx::Error::PoolTimedOut),
    }
}

#[cfg(test)]
#[path = "../../../tests/http/controllers/health.rs"]
mod tests;
