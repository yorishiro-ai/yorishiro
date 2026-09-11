use super::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

use super::fixtures::{self, TenantArgs, issue_api_key};

struct Setup {
    tenant_id: uuid::Uuid,
    owner_key: String,
    member_key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext, name: &str) -> Setup {
    let args = TenantArgs {
        tenant_name: name.into(),
        owner_email: format!("owner-{name}@example.com"),
        key_scope: ApiKeyScope::Migration,
        key_audit: false,
        ..Default::default()
    };
    let (tenant_id, workspace_id, _owner_id, owner_key) =
        fixtures::create_tenant_workspace_owner(ctx, args).await;

    let member = tenancy::create_user(
        &ctx.db,
        &format!("member-{name}@example.com"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create member");
    tenancy::add_member(&ctx.db, tenant_id, member.id, MembershipRole::Member)
        .await
        .expect("add member");
    let member_key = issue_api_key(ctx, workspace_id, member.id, ApiKeyScope::Write, false).await;

    Setup {
        tenant_id,
        owner_key,
        member_key,
    }
}

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "library-note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

#[tokio::test]
#[serial]
async fn owner_can_create_update_and_delete_a_template() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { owner_key, .. } = setup(&ctx, "acme").await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "name": "library-note",
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
        assert_eq!(body["name"], "library-note");
        assert_eq!(body["visibility"], "tenant");
        let id = body["id"].as_str().unwrap().to_string();

        let response = request
            .put(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "description": "now with a description" }))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        assert_eq!(body["description"], "now with a description");
        // A field not named in the update request must survive unchanged.
        assert_eq!(body["name"], "library-note");

        let response = request
            .delete(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 204);

        let response = request
            .get(&format!("/api/template-library/{id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn member_role_cannot_manage_the_template_library() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { member_key, .. } = setup(&ctx, "acme").await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({
                "name": "library-note",
                "definition": note_definition(),
            }))
            .await;
        assert_eq!(response.status_code(), 403);
    })
    .await;
}

/// Community visibility makes a template *readable* across tenants, not writable: only the owning tenant may update or delete it (fork creates a new row owned by the caller, so it doesn't need this guard, but update/delete do).
#[tokio::test]
#[serial]
async fn another_tenant_cannot_update_or_delete_a_community_template() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let owner_a = setup(&ctx, "acme").await;
        let owner_b = setup(&ctx, "beta").await;

        let community = yorishiro::models::_entities::template_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(owner_a.tenant_id),
            name: sea_orm::ActiveValue::Set("shared-note".into()),
            definition: sea_orm::ActiveValue::Set(note_definition()),
            visibility: sea_orm::ActiveValue::Set("community".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let community = sea_orm::ActiveModelTrait::insert(community, &ctx.db)
            .await
            .expect("insert community template");

        // Tenant B can read it (community visibility)...
        let response = request
            .get(&format!("/api/template-library/{}", community.id))
            .add_header("Authorization", format!("Bearer {}", owner_b.owner_key))
            .await;
        assert_eq!(response.status_code(), 200);

        // ...but not update it...
        let response = request
            .put(&format!("/api/template-library/{}", community.id))
            .add_header("Authorization", format!("Bearer {}", owner_b.owner_key))
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
            .delete(&format!("/api/template-library/{}", community.id))
            .add_header("Authorization", format!("Bearer {}", owner_b.owner_key))
            .await;
        assert_eq!(response.status_code(), 404);

        // The owning tenant still can.
        let response = request
            .put(&format!("/api/template-library/{}", community.id))
            .add_header("Authorization", format!("Bearer {}", owner_a.owner_key))
            .json(&serde_json::json!({ "description": "edited by the owner" }))
            .await;
        assert_eq!(response.status_code(), 200);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn fork_copies_a_community_template_into_the_forking_tenants_own_library() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let owner_a = setup(&ctx, "acme").await;
        let owner_b = setup(&ctx, "beta").await;

        let community = yorishiro::models::_entities::template_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(owner_a.tenant_id),
            name: sea_orm::ActiveValue::Set("shared-note".into()),
            definition: sea_orm::ActiveValue::Set(note_definition()),
            visibility: sea_orm::ActiveValue::Set("community".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let community = sea_orm::ActiveModelTrait::insert(community, &ctx.db)
            .await
            .expect("insert community template");

        let response = request
            .post(&format!("/api/template-library/{}/fork", community.id))
            .add_header("Authorization", format!("Bearer {}", owner_b.owner_key))
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
        assert_eq!(body["fork_of"], community.id.to_string());
        assert_eq!(body["visibility"], "tenant", "a fork starts private again");

        // Tenant B can now update its own fork.
        let fork_id = body["id"].as_str().unwrap();
        let response = request
            .put(&format!("/api/template-library/{fork_id}"))
            .add_header("Authorization", format!("Bearer {}", owner_b.owner_key))
            .json(&serde_json::json!({ "description": "tenant b's own copy" }))
            .await;
        assert_eq!(response.status_code(), 200);
    })
    .await;
}
