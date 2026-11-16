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
async fn template_update_rejects_a_revoked_previously_authenticated_actor() {
    use yorishiro::models::api_keys;
    use yorishiro::models::template_templates::{self, UpdateTemplateInput};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let created = request.post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", tenant_key))
            .json(&serde_json::json!({"name":format!("fenced-{}",uuid::Uuid::new_v4()),"definition":note_definition()})).await;
        created.assert_status(StatusCode::CREATED);
        let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().parse().unwrap();
        let key = api_keys::Entity::create_tenant_api_key(&ctx.db, tenant.tenant_id, ApiKeyScope::Read, Some(tenant.owner_id), false, "template-admin").await.unwrap();
        let actor = api_keys::AuthContext { api_key_id:key.id, workspace_id:None,
            tenant_id:tenant.tenant_id, user_id:Some(tenant.owner_id), scope:ApiKeyScope::Read, audit:false };
        template_templates::update_as(&ctx.db, &actor, id, UpdateTemplateInput {
            name: None, description: Some("before-revocation".into()), definition: None, tags: None, locale: None,
        }).await.unwrap();
        let input = || UpdateTemplateInput { name:None,description:Some("must-not-save".into()),definition:None,tags:None,locale:None };
        api_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        assert!(matches!(template_templates::update_as(&ctx.db, &actor, id, input()).await,
            Err(yorishiro::YorishiroError::Unauthenticated)));
        let stored = template_templates::get_template(&ctx.db, tenant.tenant_id, id).await.unwrap();
        assert_eq!(stored.description.as_deref(), Some("before-revocation"));
    }).await;
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
