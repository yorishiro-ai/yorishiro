use super::boot_request;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{
    identity_api_keys, identity_templates, identity_tenants, identity_workspaces,
};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: Uuid,
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
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        tenant_id: tenant.id,
        key,
    }
}

async fn insert_template(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    definition: serde_json::Value,
) -> identity_templates::Model {
    let template = identity_templates::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant_id),
        name: sea_orm::ActiveValue::Set("library-note".into()),
        definition: sea_orm::ActiveValue::Set(definition),
        visibility: sea_orm::ActiveValue::Set("tenant".into()),
        tags: sea_orm::ActiveValue::Set(vec![]),
        ..Default::default()
    };
    sea_orm::ActiveModelTrait::insert(template, &ctx.db)
        .await
        .expect("insert library template")
}

fn note_definition() -> serde_json::Value {
    json!({
        "name": "library-note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

/// A schema with no origin template reports nothing to follow, and merge-preview/merge both refuse it: the whole point of the origin/merge chain only applies to a schema copied from a template.
#[tokio::test]
#[serial]
async fn a_schema_with_no_origin_is_never_reported_or_mergeable() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&note_definition())
            .await;
        assert_eq!(create.status_code(), 201, "response: {:?}", create.text());
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let changes = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(changes.status_code(), 200);
        assert!(
            changes.json::<Vec<serde_json::Value>>().is_empty(),
            "a schema with no origin template must never be reported"
        );

        let preview = request
            .get(&format!("/api/schemas/{schema_id}/merge-preview"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(preview.status_code(), 422, "response: {:?}", preview.text());
    })
    .await;
}

/// The full round trip: a schema copied from a template, the template edited afterward, the change surfacing in the upstream-changes listing and the merge preview, and merge writing a new version that both takes upstream's addition and keeps the workspace's own field.
#[tokio::test]
#[serial]
async fn upstream_changes_preview_and_merge_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        let template = insert_template(&ctx, setup.tenant_id, note_definition()).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({ "template_id": template.id.to_string() }))
            .await;
        assert_eq!(create.status_code(), 201, "response: {:?}", create.text());

        // Not yet reported: the template has not moved since the copy was taken.
        let before = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert!(before.json::<Vec<serde_json::Value>>().is_empty());

        // Add a field to the workspace's own copy, so the merge has something local to keep.
        // This is a second version of the same name (create_schema archives the first and installs this as the new active row), so it has its own id: the origin/merge chain always acts on the currently active version, which is this one from here on.
        let edit_local = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({
                "name": "library-note",
                "entity_types": {
                    "note": {
                        "fields": {
                            "title": { "type": "string", "required": true },
                            "internal_ref": { "type": "string" }
                        }
                    }
                }
            }))
            .await;
        assert_eq!(
            edit_local.status_code(),
            201,
            "response: {:?}",
            edit_local.text()
        );
        let schema_id: Uuid = edit_local.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        // Edit the template upstream: add a field the workspace does not have.
        let mut active: identity_templates::ActiveModel = template.clone().into();
        active.definition = sea_orm::ActiveValue::Set(json!({
            "name": "library-note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "category": { "type": "string" }
                    }
                }
            }
        }));
        active.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
        sea_orm::ActiveModelTrait::update(active, &ctx.db)
            .await
            .expect("update template");

        let after = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let after_body: Vec<serde_json::Value> = after.json();
        assert_eq!(
            after_body.len(),
            1,
            "the edited template must now be reported: {after_body:?}"
        );
        assert_eq!(after_body[0]["schema_id"], schema_id.to_string());

        let preview = request
            .get(&format!("/api/schemas/{schema_id}/merge-preview"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(preview.status_code(), 200, "response: {:?}", preview.text());
        let plan: serde_json::Value = preview.json();
        let fields = plan["fields"].as_array().unwrap();
        assert!(
            fields
                .iter()
                .any(|f| f["field"] == "category" && f["verdict"] == "auto_add"),
            "upstream's addition must be in the plan: {fields:?}"
        );
        assert!(
            fields
                .iter()
                .any(|f| f["field"] == "internal_ref" && f["verdict"] == "keep_local"),
            "the workspace's own field is reported as kept, not silently dropped: {fields:?}"
        );

        let merge = request
            .post(&format!("/api/schemas/{schema_id}/merge"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(merge.status_code(), 201, "response: {:?}", merge.text());
        let merge_body: serde_json::Value = merge.json();
        let merged_fields = &merge_body["schema"]["definition"]["entity_types"]["note"]["fields"];
        assert!(
            merged_fields.get("category").is_some(),
            "upstream's addition must land: {merged_fields:?}"
        );
        assert!(
            merged_fields.get("internal_ref").is_some(),
            "the workspace's own field must survive: {merged_fields:?}"
        );
        assert_eq!(merge_body["schema"]["version"], 3);
    })
    .await;
}

/// Merging a schema whose two sides conflict on the same field is refused rather than picking one side silently.
#[tokio::test]
#[serial]
async fn merging_a_conflicting_field_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        let base_def = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "priority": { "type": "string" } } }
            }
        });
        let template = insert_template(&ctx, setup.tenant_id, base_def.clone()).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({ "template_id": template.id.to_string() }))
            .await;
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({
                "name": "library-note",
                "entity_types": {
                    "note": { "fields": { "priority": { "type": "boolean" } } }
                }
            }))
            .await;

        let mut active: identity_templates::ActiveModel = template.clone().into();
        active.definition = sea_orm::ActiveValue::Set(json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "priority": { "type": "integer" } } }
            }
        }));
        active.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
        sea_orm::ActiveModelTrait::update(active, &ctx.db)
            .await
            .expect("update template");

        let merge = request
            .post(&format!("/api/schemas/{schema_id}/merge"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(merge.status_code(), 422, "response: {:?}", merge.text());
    })
    .await;
}
