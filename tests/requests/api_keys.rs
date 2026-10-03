use axum::http::StatusCode;
use serde_json::Value;
use uuid::Uuid;
use yorishiro::app::App;

use super::boot_request;
use super::fixtures::{self, TenantArgs};

#[tokio::test]
async fn api_key_lifecycle_is_admin_only_and_secret_is_one_time() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id, owner_id, owner_key) =
            fixtures::create_tenant_workspace_owner(&ctx, TenantArgs::default()).await;

        assert_eq!(
            request.get("/api/api-keys").await.status_code(),
            StatusCode::UNAUTHORIZED
        );

        let created = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "name": "  my-client  " }))
            .await;
        assert_eq!(created.status_code(), StatusCode::CREATED);
        let created_body: Value = created.json();
        assert_eq!(created_body["name"], "my-client");
        assert!(created_body["full_key"].as_str().is_some());
        assert!(created_body["prefix"].as_str().unwrap().starts_with("ysr_"));
        assert!(created_body["id"].as_str().is_some());
        assert!(created_body["created_at"].as_str().is_some());

        let full_key = created_body["full_key"].as_str().unwrap().to_owned();
        let listed = request
            .get("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed_body: Value = listed.json();
        assert_eq!(listed_body["keys"].as_array().unwrap().len(), 2);
        assert!(listed_body.to_string().contains("my-client"));
        assert!(!listed_body.to_string().contains(&full_key));
        assert!(!listed_body.to_string().contains("key_hash"));

        let authenticated = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(authenticated.status_code(), StatusCode::OK);

        let key_id: Uuid = created_body["id"].as_str().unwrap().parse().unwrap();
        let revoked = request
            .delete(&format!("/api/api-keys/{key_id}"))
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(revoked.status_code(), StatusCode::NO_CONTENT);

        let no_longer_authenticated = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(
            no_longer_authenticated.status_code(),
            StatusCode::UNAUTHORIZED
        );

        assert_eq!(
            request
                .delete(&format!("/api/api-keys/{key_id}"))
                .add_header("Authorization", format!("Bearer {owner_key}"))
                .await
                .status_code(),
            StatusCode::NOT_FOUND
        );

        let invalid = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "name": "   " }))
            .await;
        assert_eq!(invalid.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

        let missing = request
            .delete("/api/api-keys/not-a-uuid")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(missing.status_code(), StatusCode::BAD_REQUEST);

        let _ = (tenant_id, workspace_id, owner_id);
    })
    .await;
}

#[tokio::test]
async fn api_key_lifecycle_rejects_non_admin_and_cross_tenant_ids() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant_id, workspace_id, _, owner_key) =
            fixtures::create_tenant_workspace_owner(&ctx, TenantArgs::default()).await;
        let member = yorishiro::models::tenancy::create_user(
            &ctx.db,
            "member@example.com",
            "member-password",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenancy::add_member(
            &ctx.db,
            tenant_id,
            member.id,
            yorishiro::models::tenancy::MembershipRole::Member,
        )
        .await
        .expect("add member");
        let member_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            workspace_id,
            yorishiro::models::api_keys::ApiKeyScope::Read,
            Some(member.id),
            false,
        )
        .await
        .expect("issue member key")
        .plaintext;
        let forbidden = request
            .get("/api/api-keys")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(forbidden.status_code(), StatusCode::FORBIDDEN);

        let (_, other_workspace, _, other_key) = fixtures::create_tenant_workspace_owner(
            &ctx,
            TenantArgs {
                tenant_name: "other".into(),
                workspace_name: "other".into(),
                owner_email: "other@example.com".into(),
                ..TenantArgs::default()
            },
        )
        .await;

        let other_created = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {other_key}"))
            .json(&serde_json::json!({ "name": "other-client" }))
            .await;
        assert_eq!(other_created.status_code(), StatusCode::CREATED);
        let other_id = other_created.json::<Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let cross_tenant = request
            .delete(&format!("/api/api-keys/{other_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(cross_tenant.status_code(), StatusCode::NOT_FOUND);

        let malformed_workspace = request
            .delete(&format!("/api/api-keys/{other_workspace}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(malformed_workspace.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}
