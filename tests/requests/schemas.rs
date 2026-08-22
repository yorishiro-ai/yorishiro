use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro_core::app::App;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: uuid::Uuid,
    key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
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
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        tenant_id: tenant.id,
        key,
    }
}

#[tokio::test]
#[serial]
async fn create_schema_from_a_builtin_template() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { key, .. } = setup(&ctx).await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(response.status_code(), 201);
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["name"], "task-management");
        assert!(
            body["schema"]["definition"]["entity_types"]["task"].is_object(),
            "body: {body}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn create_schema_rejects_an_unknown_template_id() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { key, .. } = setup(&ctx).await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "no-such-template" }))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn list_templates_and_get_template_over_rest() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { key, .. } = setup(&ctx).await;

        let response = request
            .get("/api/templates")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        let ids: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"task-management"), "ids: {ids:?}");

        let response = request
            .get("/api/templates/task-management")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), 200);
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "task-management");

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// The reason D2 exists: a UUID `template_id` names a row in the tenant's own library, not a
/// built-in. Every other test in this file only exercises the built-in branch of
/// resolve_template_definition (Uuid::parse_str fails -> crate::templates::get_template); this
/// is the only one that exercises the library branch (parse succeeds ->
/// identity_templates::get_template), which is what stamps origin_template_id/origin_status/
/// origin_snapshot onto the created schema.
#[tokio::test]
#[serial]
async fn create_schema_from_a_library_template_links_the_origin() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { tenant_id, key } = setup(&ctx).await;

        let template = yorishiro_core::models::_entities::identity_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant_id),
            name: sea_orm::ActiveValue::Set("library-note".into()),
            definition: sea_orm::ActiveValue::Set(serde_json::json!({
                "name": "library-note",
                "entity_types": {
                    "note": { "fields": { "title": { "type": "string", "required": true } } }
                }
            })),
            visibility: sea_orm::ActiveValue::Set("tenant".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let template = sea_orm::ActiveModelTrait::insert(template, &ctx.db)
            .await
            .expect("insert library template");

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": template.id.to_string() }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["name"], "library-note");
        assert_eq!(
            body["schema"]["origin_template_id"],
            template.id.to_string()
        );
        assert_eq!(body["schema"]["origin_status"], "linked");
        assert!(
            body["schema"]["origin_snapshot"].is_object(),
            "body: {body}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A caller passing no origin on a second version must not silently un-link a schema that was
/// created from a template: content_schemas::create_schema inherits the previous active
/// version's origin when the caller passes None, matching master's create_schema_with_base
/// intent (this codebase's simpler port does not carry the three-way merge base, only the
/// origin identity/status/snapshot).
#[tokio::test]
#[serial]
async fn a_second_version_with_no_origin_inherits_the_first_versions_link() {
    request_with_create_db::<App, _, _>(|request, ctx| async move {
        let Setup { tenant_id, key } = setup(&ctx).await;

        let template = yorishiro_core::models::_entities::identity_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant_id),
            name: sea_orm::ActiveValue::Set("library-note".into()),
            definition: sea_orm::ActiveValue::Set(serde_json::json!({
                "name": "library-note",
                "entity_types": {
                    "note": { "fields": { "title": { "type": "string", "required": true } } }
                }
            })),
            visibility: sea_orm::ActiveValue::Set("tenant".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let template = sea_orm::ActiveModelTrait::insert(template, &ctx.db)
            .await
            .expect("insert library template");

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": template.id.to_string() }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );

        // A second version, posted as an inline definition (no template_id at all): the REST
        // body shape has no way to say "same origin as before", so this is exactly the case an
        // operator hits editing a linked schema by hand.
        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({
                "name": "library-note",
                "entity_types": {
                    "note": {
                        "fields": {
                            "title": { "type": "string", "required": true },
                            "body": { "type": "string", "required": false }
                        }
                    }
                }
            }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["version"], 2);
        assert_eq!(
            body["schema"]["origin_template_id"],
            template.id.to_string(),
            "the link to the library template must survive an edit with no explicit origin: {body}"
        );
        assert_eq!(body["schema"]["origin_status"], "linked");

        super::close_app_pools(&ctx).await;
    })
    .await;
}
