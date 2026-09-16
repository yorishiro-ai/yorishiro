use super::boot_request;
use axum::http::StatusCode;
use sea_orm::TransactionTrait;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{
    api_keys, template_templates, tenant_tenants, workspace_workspaces,
};
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: Uuid,
    workspace_id: Uuid,
    key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = tenant_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = workspace_workspaces::ActiveModel {
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
    let key = api_keys::Entity::create_api_key(
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
        workspace_id: workspace.id,
        key,
    }
}

async fn insert_template(
    ctx: &loco_rs::app::AppContext,
    tenant_id: Uuid,
    definition: serde_json::Value,
) -> template_templates::Model {
    let template = template_templates::ActiveModel {
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
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&note_definition())
            .await;
        assert_eq!(create.status_code(), StatusCode::CREATED, "response: {:?}", create.text());
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let changes = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(changes.status_code(), StatusCode::OK);
        assert!(
            changes.json::<Vec<serde_json::Value>>().is_empty(),
            "a schema with no origin template must never be reported"
        );

        let preview = request
            .get(&format!("/api/schemas/{schema_id}/merge-preview"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(preview.status_code(), StatusCode::UNPROCESSABLE_ENTITY, "response: {:?}", preview.text());
    })
    .await;
}

/// The full round trip: a schema copied from a template, the template edited afterward, the change surfacing in the upstream-changes listing and the merge preview, and merge writing a new version that both takes upstream's addition and keeps the workspace's own field.
#[tokio::test]
#[serial]
async fn upstream_changes_preview_and_merge_round_trip() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;
        let template = insert_template(&ctx, setup.tenant_id, note_definition()).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({ "template_id": template.id.to_string() }))
            .await;
        assert_eq!(create.status_code(), StatusCode::CREATED, "response: {:?}", create.text());

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
        // Edit the template upstream: add a field the workspace does not have.
        let updated_definition: yorishiro::metaschema::MetaSchemaDefinition =
            serde_json::from_value(serde_json::json!({
            "name": "library-note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "category": { "type": "string" }
                    }
                }
            }
            }))
            .expect("parse updated definition");
        let template_txn = ctx.db.begin().await.expect("begin template update");
        yorishiro::models::template_templates::update_template(
            &template_txn,
            setup.tenant_id,
            template.id,
            yorishiro::models::template_templates::UpdateTemplateInput {
                name: None,
                description: None,
                definition: Some(updated_definition.clone()),
                tags: None,
                locale: None,
            },
        )
        .await
        .expect("mark linked schema pending");
        template_txn.commit().await.expect("commit template update");
        let repeat_txn = ctx.db.begin().await.expect("begin repeated update");
        yorishiro::models::template_templates::update_template(
            &repeat_txn,
            setup.tenant_id,
            template.id,
            yorishiro::models::template_templates::UpdateTemplateInput {
                name: None,
                description: None,
                definition: Some(updated_definition),
                tags: None,
                locale: None,
            },
        )
        .await
        .expect("repeat pending mark");
        repeat_txn.commit().await.expect("commit repeated update");

        // A local version created after publication inherits the pending state instead of acknowledging it.
        let local_after_publication = request
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
            local_after_publication.status_code(),
            201,
            "response: {:?}",
            local_after_publication.text()
        );
        let schema_id: Uuid = local_after_publication.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

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
        assert_eq!(after_body[0]["pending_notification"], true);
        assert_eq!(after_body[0]["summary"]["total_fields"], 2);
        assert_eq!(after_body[0]["summary"]["auto_add"], 1);
        assert_eq!(after_body[0]["summary"]["keep_local"], 1);

        let preview = request
            .get(&format!("/api/schemas/{schema_id}/merge-preview"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(preview.status_code(), StatusCode::OK, "response: {:?}", preview.text());
        let plan: serde_json::Value = preview.json();
        let fields = plan["fields"].as_array().unwrap();
        assert!(
            fields
                .iter()
                .any(|f| f["field"] == "category" && f["verdict"] == "auto_add"),
            "upstream's addition must be in the plan: {fields:?}"
        );
        assert_eq!(plan["summary"]["total_fields"], 2);
        assert_eq!(plan["summary"]["auto_add"], 1);
        assert_eq!(plan["summary"]["keep_local"], 1);
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
        assert_eq!(merge.status_code(), StatusCode::CREATED, "response: {:?}", merge.text());
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
        assert_eq!(merge_body["schema"]["version"], 4);
        assert_eq!(merge_body["summary"]["total_fields"], 2);
        assert_eq!(merge_body["summary"]["has_conflicts"], false);

        let acknowledged = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert!(acknowledged.json::<Vec<serde_json::Value>>().is_empty());
    })
    .await;
}

/// A publication racing with merge cannot be acknowledged by the merge that read the older revision.
#[tokio::test]
#[serial]
async fn publication_waits_for_merge_revision_lock_and_remains_pending() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        struct ReadBarrier {
            ready: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }

        #[async_trait::async_trait]
        impl yorishiro::ee::services::origin::MergeReadHook for ReadBarrier {
            async fn after_revision_read(&self) {
                self.ready.notify_one();
                self.release.notified().await;
            }
        }

        let setup = setup(&ctx).await;
        let template = insert_template(&ctx, setup.tenant_id, note_definition()).await;
        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&json!({ "template_id": template.id.to_string() }))
            .await;
        assert_eq!(create.status_code(), StatusCode::CREATED);
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let first_update = ctx.db.begin().await.expect("begin first publication");
        yorishiro::models::template_templates::update_template(
            &first_update,
            setup.tenant_id,
            template.id,
            yorishiro::models::template_templates::UpdateTemplateInput {
                name: None,
                description: None,
                definition: Some(
                    serde_json::from_value(json!({
                        "name": "library-note",
                        "entity_types": { "note": { "fields": {
                            "title": { "type": "string", "required": true },
                            "category": { "type": "string" }
                        } } }
                    }))
                    .unwrap(),
                ),
                tags: None,
                locale: None,
            },
        )
        .await
        .expect("publish first revision");
        first_update
            .commit()
            .await
            .expect("commit first publication");

        let barrier = std::sync::Arc::new(ReadBarrier {
            ready: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let merge_db = ctx.db.clone();
        let merge_ctx = ctx.clone();
        let tenant_id = setup.tenant_id;
        let workspace_id = setup.workspace_id;
        let merge_barrier = barrier.clone();
        let merge = tokio::spawn(async move {
            let txn = merge_db.begin().await.expect("begin merge transaction");
            let result = yorishiro::ee::services::origin::merge_apply_with_read_hook(
                &txn,
                &merge_ctx,
                tenant_id,
                workspace_id,
                schema_id,
                merge_barrier.as_ref(),
            )
            .await;
            if result.is_ok() {
                txn.commit().await.expect("commit merge acknowledgement");
            } else {
                txn.rollback().await.expect("rollback failed merge");
            }
            result
        });
        barrier.ready.notified().await;

        let update_db = ctx.db.clone();
        let mut update = tokio::spawn(async move {
            let txn = update_db
                .begin()
                .await
                .expect("begin concurrent publication");
            let result = yorishiro::models::template_templates::update_template(
                &txn,
                setup.tenant_id,
                template.id,
                yorishiro::models::template_templates::UpdateTemplateInput {
                    name: None,
                    description: None,
                    definition: Some(
                        serde_json::from_value(json!({
                            "name": "library-note",
                            "entity_types": { "note": { "fields": {
                                "title": { "type": "string", "required": true },
                                "category": { "type": "string" },
                                "status": { "type": "string" }
                            } } }
                        }))
                        .unwrap(),
                    ),
                    tags: None,
                    locale: None,
                },
            )
            .await;
            if result.is_ok() {
                txn.commit().await.expect("commit concurrent publication");
            }
            result
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut update)
                .await
                .is_err(),
            "publication must wait while merge owns the revision lock"
        );
        barrier.release.notify_one();
        let merge_result = merge.await.expect("merge task").expect("merge application");
        assert_eq!(merge_result.0.version, 2);
        update
            .await
            .expect("concurrent publication task")
            .expect("concurrent publication");

        let changes = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let body: Vec<serde_json::Value> = changes.json();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0]["pending_notification"], true);
        assert_eq!(body[0]["summary"]["auto_add"], 1);
    })
    .await;
}

/// Merging a schema whose two sides conflict on the same field is refused rather than picking one side silently.
#[tokio::test]
#[serial]
async fn merging_a_conflicting_field_is_refused() {
    if super::super::require_sqlite_backend() {
        return;
    }
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

        let template_txn = ctx.db.begin().await.expect("begin template update");
        yorishiro::models::template_templates::update_template(
            &template_txn,
            setup.tenant_id,
            template.id,
            yorishiro::models::template_templates::UpdateTemplateInput {
                name: None,
                description: None,
                definition: Some(
                    serde_json::from_value(json!({
                        "name": "library-note",
                        "entity_types": {
                            "note": { "fields": { "priority": { "type": "integer" } } }
                        }
                    }))
                    .unwrap(),
                ),
                tags: None,
                locale: None,
            },
        )
        .await
        .expect("update template");
        template_txn.commit().await.expect("commit template update");

        let merge = request
            .post(&format!("/api/schemas/{schema_id}/merge"))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(merge.status_code(), StatusCode::UNPROCESSABLE_ENTITY, "response: {:?}", merge.text());
        let pending = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let pending_body: Vec<serde_json::Value> = pending.json();
        assert_eq!(pending_body.len(), 1);
        assert_eq!(pending_body[0]["pending_notification"], true);
        assert_eq!(pending_body[0]["summary"]["conflict"], 1);
    })
    .await;
}
