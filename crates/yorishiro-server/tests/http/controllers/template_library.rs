use crate::build_app;
use crate::test_support::*;
use axum::http::StatusCode;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::models::tenancy;

fn sample_definition(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        },
    })
}

/// Creates a tenant with one workspace and one member holding `role`, and returns `(tenant_id, api_key)` for that member.
/// The four write endpoints below gate on the caller's *membership role* (`require_tenant_admin`), not on the API key's scope, so every caller here gets `role.max_scope()` and the role is what varies between tests.
async fn seed_member(pool: &PgPool, email: &str, role: tenancy::MembershipRole) -> (Uuid, String) {
    let tenant = tenancy::create_tenant(pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(pool, tenant.id, "main", None, None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut conn, email, "hunter2-hunter2", None)
        .await
        .unwrap();
    tenancy::add_member(&mut conn, tenant.id, user.id, role)
        .await
        .unwrap();
    drop(conn);

    let key = issue_key_for(pool, tenant.id, workspace.id, user.id, role).await;
    (tenant.id, key)
}

/// The happy path for all four admin-gated endpoints.
/// Without this, an implementation that returned 403 unconditionally would still satisfy every rejection test below.
#[sqlx::test(migrations = "../../migrations")]
async fn owner_can_create_update_fork_and_delete_a_template(pool: PgPool) {
    let (_tenant_id, owner_key) =
        seed_member(&pool, "owner@example.com", tenancy::MembershipRole::Owner).await;
    let owner_auth = format!("Bearer {owner_key}");

    let app = build_app(test_state(pool), no_static_fallback());

    let response = rest_request(
        &app,
        "POST",
        "/api/template-library",
        Some(&owner_auth),
        Some(serde_json::json!({
            "name": "notes",
            "definition": sample_definition("notes"),
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = rest_json_body(response).await;
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "notes");

    let response = rest_request(
        &app,
        "PUT",
        &format!("/api/template-library/{id}"),
        Some(&owner_auth),
        Some(serde_json::json!({ "description": "field notes" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let updated = rest_json_body(response).await;
    assert_eq!(updated["description"], "field notes");

    let response = rest_request(
        &app,
        "POST",
        &format!("/api/template-library/{id}/fork"),
        Some(&owner_auth),
        Some(serde_json::json!({ "name": "notes-fork" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let forked = rest_json_body(response).await;
    assert_eq!(forked["name"], "notes-fork");
    assert_ne!(forked["id"], created["id"]);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/template-library/{id}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/template-library/{id}"),
        Some(&owner_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Reading is open to any member; only the four write endpoints are admin-gated.
#[sqlx::test(migrations = "../../migrations")]
async fn member_role_can_read_but_cannot_write_templates(pool: PgPool) {
    let (tenant_id, member_key) =
        seed_member(&pool, "member@example.com", tenancy::MembershipRole::Member).await;
    let member_auth = format!("Bearer {member_key}");

    // Seeded past the HTTP layer, since a member can't create one through the API.
    let existing = tenancy::create_template(
        &pool,
        tenant_id,
        None,
        tenancy::CreateTemplateInput {
            name: "seeded".to_string(),
            description: None,
            definition: serde_json::from_value(sample_definition("seeded")).unwrap(),
            tags: Vec::new(),
            locale: None,
            author: None,
        },
    )
    .await
    .unwrap();

    let app = build_app(test_state(pool), no_static_fallback());

    let response = rest_request(
        &app,
        "GET",
        "/api/template-library",
        Some(&member_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = rest_request(
        &app,
        "GET",
        &format!("/api/template-library/{}", existing.id),
        Some(&member_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = rest_request(
        &app,
        "POST",
        "/api/template-library",
        Some(&member_auth),
        Some(serde_json::json!({
            "name": "nope",
            "definition": sample_definition("nope"),
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = rest_request(
        &app,
        "PUT",
        &format!("/api/template-library/{}", existing.id),
        Some(&member_auth),
        Some(serde_json::json!({ "description": "nope" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = rest_request(
        &app,
        "POST",
        &format!("/api/template-library/{}/fork", existing.id),
        Some(&member_auth),
        Some(serde_json::json!({ "name": "nope-fork" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/template-library/{}", existing.id),
        Some(&member_auth),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "../../migrations")]
async fn template_library_endpoints_require_authentication(pool: PgPool) {
    let app = build_app(test_state(pool), no_static_fallback());
    let id = Uuid::nil();

    let response = rest_request(&app, "GET", "/api/template-library", None, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = rest_request(
        &app,
        "POST",
        "/api/template-library",
        None,
        Some(serde_json::json!({
            "name": "nope",
            "definition": sample_definition("nope"),
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = rest_request(
        &app,
        "PUT",
        &format!("/api/template-library/{id}"),
        None,
        Some(serde_json::json!({ "description": "nope" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = rest_request(
        &app,
        "DELETE",
        &format!("/api/template-library/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = rest_request(
        &app,
        "POST",
        &format!("/api/template-library/{id}/fork"),
        None,
        Some(serde_json::json!({ "name": "nope-fork" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
