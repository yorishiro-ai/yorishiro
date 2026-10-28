use super::boot_request;
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;

use super::fixtures::{self, TenantArgs, TestTenant, issue_api_key};

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "library-note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

#[tokio::test]
async fn owner_can_create_update_and_delete_a_template() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let name = format!("library-note-{}", uuid::Uuid::new_v4().simple());
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "name": name,
                "definition": note_definition(),
                "tags": ["scratch"],
            }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], name);
        assert_eq!(body["visibility"], "tenant");
        let id = body["id"].as_str().unwrap().to_string();

        let response = request
            .put(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "description": "now with a description" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["description"], "now with a description");
        // A field not named in the update request must survive unchanged.
        assert_eq!(body["name"], name);

        let response = request
            .delete(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::NO_CONTENT);

        let response = request
            .get(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn member_role_cannot_manage_the_template_library() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &fixtures::unique_email("member"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            member.id,
            MembershipRole::Member,
        )
        .await
        .expect("add member");
        super::fixtures::grant_workspace(
            &ctx,
            tenant.workspace_id,
            member.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Member,
        )
        .await;
        let member_key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            member.id,
            ApiKeyScope::Write,
            false,
        )
        .await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({
                "name": "library-note",
                "definition": note_definition(),
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

/// Community visibility makes a template *readable* across tenants, not writable: only the owning tenant may update or delete it (fork creates a new row owned by the caller, so it doesn't need this guard, but update/delete do).
#[tokio::test]
#[cfg(feature = "enterprise")]
async fn another_tenant_cannot_update_or_delete_a_community_template() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let owner_a = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let owner_b = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner_a.api_key))
            .json(&serde_json::json!({
                "name": "shared-note",
                "definition": note_definition(),
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
        let id: uuid::Uuid =
            serde_json::from_value(response.json::<serde_json::Value>()["id"].clone())
                .expect("created template ID");
        request
            .put(&format!("/api/marketplace/{id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner_a.api_key))
            .json(&serde_json::json!({"visibility": "community"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        // Tenant B can read it (community visibility)...
        let response = request
            .get(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {}", owner_b.api_key))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);

        // ...but not update it...
        let response = request
            .put(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {}", owner_b.api_key))
            .json(&serde_json::json!({ "description": "hijacked" }))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );

        // ...nor delete it.
        let response = request
            .delete(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {}", owner_b.api_key))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);

        // The owning tenant still can.
        let response = request
            .put(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {}", owner_a.api_key))
            .json(&serde_json::json!({ "description": "edited by the owner" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn fork_copies_a_community_template_into_the_forking_tenants_own_library() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let owner_a = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let owner_b = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner_a.api_key))
            .json(&serde_json::json!({
                "name": "shared-note",
                "definition": note_definition(),
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
        let id: uuid::Uuid =
            serde_json::from_value(response.json::<serde_json::Value>()["id"].clone())
                .expect("created template ID");
        request
            .put(&format!("/api/marketplace/{id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner_a.api_key))
            .json(&serde_json::json!({"visibility": "community"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let response = request
            .post(&format!("/api/template-library/{id}/fork"))
            .add_header("Authorization", format!("Bearer {}", owner_b.api_key))
            .json(&serde_json::json!({ "name": "my-copy" }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "my-copy");
        assert_eq!(body["tenant_id"], owner_b.tenant_id.to_string());
        assert_eq!(body["fork_of"], id.to_string());
        assert_eq!(body["visibility"], "tenant", "a fork starts private again");

        // Tenant B can now update its own fork.
        let fork_id = body["id"].as_str().unwrap();
        let response = request
            .put(&format!("/api/template-library/{fork_id}"))
            .add_header("Authorization", format!("Bearer {}", owner_b.api_key))
            .json(&serde_json::json!({ "description": "tenant b's own copy" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
    })
    .await;
}
