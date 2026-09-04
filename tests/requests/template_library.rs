use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: uuid::Uuid,
    owner_key: String,
    member_key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext, name: &str) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");

    let owner = tenancy::create_user(
        &ctx.db,
        &format!("owner-{name}@example.com"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let owner_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Migration,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue owner key")
    .plaintext;

    let member = tenancy::create_user(
        &ctx.db,
        &format!("member-{name}@example.com"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create member");
    tenancy::add_member(&ctx.db, tenant.id, member.id, MembershipRole::Member)
        .await
        .expect("add member");
    let member_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Write,
        Some(member.id),
        false,
    )
    .await
    .expect("issue member key")
    .plaintext;

    Setup {
        tenant_id: tenant.id,
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
    request_with_create_db::<App, _, _>(|request, ctx| async move {
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
    request_with_create_db::<App, _, _>(|request, ctx| async move {
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
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let owner_a = setup(&ctx, "acme").await;
        let owner_b = setup(&ctx, "beta").await;

        let community = yorishiro::models::_entities::identity_templates::ActiveModel {
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
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let owner_a = setup(&ctx, "acme").await;
        let owner_b = setup(&ctx, "beta").await;

        let community = yorishiro::models::_entities::identity_templates::ActiveModel {
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
