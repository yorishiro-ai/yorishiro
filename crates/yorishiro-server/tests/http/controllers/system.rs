use crate::build_app;
use crate::test_support::*;
use axum::http::StatusCode;
use sqlx::PgPool;
use yorishiro_core::models::tenancy;

async fn get_json(app: &axum::Router, path: &str, key: &str) -> serde_json::Value {
    let response = rest_request(app, "GET", path, Some(&format!("Bearer {key}")), None).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    rest_json_body(response).await
}

async fn get_status(app: &axum::Router, path: &str, key: &str) -> StatusCode {
    rest_request(app, "GET", path, Some(&format!("Bearer {key}")), None)
        .await
        .status()
}

async fn put_json(
    app: &axum::Router,
    path: &str,
    key: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = rest_request(app, "PUT", path, Some(&format!("Bearer {key}")), Some(body)).await;
    let status = response.status();
    (status, rest_json_body(response).await)
}

/// An owner's key carries `migration` scope, which is what guards these routes.
async fn owner_key(pool: &PgPool) -> String {
    let tenant = tenancy::create_tenant(pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(pool, tenant.id, "main", None, None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let owner = tenancy::create_user(&mut *conn, "owner@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(
        &mut *conn,
        tenant.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await
    .unwrap();
    drop(conn);
    issue_key_for(
        pool,
        tenant.id,
        workspace.id,
        owner.id,
        tenancy::MembershipRole::Owner,
    )
    .await
}

/// A member's key tops out at `write`, which is below `migration`.
async fn member_key(pool: &PgPool) -> String {
    let tenant = tenancy::create_tenant(pool, "beta", None).await.unwrap();
    let workspace = tenancy::create_workspace(pool, tenant.id, "main", None, None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let member = tenancy::create_user(&mut *conn, "member@example.com", "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(
        &mut *conn,
        tenant.id,
        member.id,
        tenancy::MembershipRole::Member,
    )
    .await
    .unwrap();
    drop(conn);
    issue_key_for(
        pool,
        tenant.id,
        workspace.id,
        member.id,
        tenancy::MembershipRole::Member,
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn maintenance_is_readable_and_settable_over_rest(pool: PgPool) {
    let key = owner_key(&pool).await;
    let app = build_app(test_state(pool.clone()), no_static_fallback());

    let body = get_json(&app, "/api/system/maintenance", &key).await;
    assert_eq!(body["mode"], "off", "a fresh deployment serves normally");

    let (status, body) = put_json(
        &app,
        "/api/system/maintenance",
        &key,
        serde_json::json!({ "mode": "read-only", "reason": "restoring a backup" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["mode"], "read_only");
    assert_eq!(body["reason"], "restoring a backup");
    assert_eq!(body["retry_after"], 300, "the CLI's default, unchanged");

    let body = get_json(&app, "/api/system/maintenance", &key).await;
    assert_eq!(body["mode"], "read_only", "the write is what the read sees");
}

/// The point of the endpoint.
/// Behind the maintenance guard, a full lock entered over REST could only be left over the CLI, which makes the switch a one-way door for anyone without shell access.
/// This test fails if the route ever moves behind the guard.
#[sqlx::test(migrations = "../../migrations")]
async fn a_full_lock_entered_over_rest_can_be_left_over_rest(pool: PgPool) {
    let key = owner_key(&pool).await;
    let app = build_app(test_state(pool.clone()), no_static_fallback());

    let (status, _) = put_json(
        &app,
        "/api/system/maintenance",
        &key,
        serde_json::json!({ "mode": "full-lock" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Everything else is refused now, which is what full lock means.
    let locked = get_status(&app, "/api/workspaces", &key).await;
    assert_eq!(locked, StatusCode::SERVICE_UNAVAILABLE);

    // The switch itself still answers, or there would be no way back.
    let (status, body) = put_json(
        &app,
        "/api/system/maintenance",
        &key,
        serde_json::json!({ "mode": "off" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the maintenance route must answer under full lock"
    );
    assert_eq!(body["mode"], "off");

    let served = get_status(&app, "/api/workspaces", &key).await;
    assert_eq!(served, StatusCode::OK, "and the deployment is back");
}

/// `migration` is the scope that guards batch migration and the maintenance switch alike.
/// A `write`-scoped key stopping every caller would be an escalation.
#[sqlx::test(migrations = "../../migrations")]
async fn a_member_key_cannot_touch_maintenance(pool: PgPool) {
    let key = member_key(&pool).await;
    let app = build_app(test_state(pool.clone()), no_static_fallback());

    assert_eq!(
        get_status(&app, "/api/system/maintenance", &key).await,
        StatusCode::FORBIDDEN
    );

    let (status, _) = put_json(
        &app,
        "/api/system/maintenance",
        &key,
        serde_json::json!({ "mode": "full-lock" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// A typo must not read as a mode.
/// Silently ignoring it would leave an operator believing the deployment is locked when it is serving.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unknown_mode_is_refused(pool: PgPool) {
    let key = owner_key(&pool).await;
    let app = build_app(test_state(pool.clone()), no_static_fallback());

    let (status, body) = put_json(
        &app,
        "/api/system/maintenance",
        &key,
        serde_json::json!({ "mode": "readonly" }),
    )
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        body["error"]["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("read-only"),
        "the hint must spell the modes: {body}"
    );
}

/// The endpoint has to appear in the published document, or a client generating from it will not know the switch exists.
/// `utoipa` only includes what `paths(...)` names, and forgetting that registration produces a working endpoint nobody can discover.
#[test]
fn maintenance_is_in_the_openapi_document() {
    use utoipa::OpenApi;

    let doc = serde_json::to_value(crate::http::controllers::ApiDoc::openapi()).unwrap();
    let path = &doc["paths"]["/api/system/maintenance"];

    assert!(path.is_object(), "the path is missing from the document");
    assert!(path["get"].is_object(), "GET is not documented");
    assert!(path["put"].is_object(), "PUT is not documented");
}
