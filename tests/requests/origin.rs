use super::boot_request;
use axum::http::StatusCode;
use sea_orm::TransactionTrait;
use serde_json::json;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use super::fixtures::{TenantArgs, TestTenant};

async fn create_template(
    request: &loco_rs::TestServer,
    key: &str,
    definition: serde_json::Value,
) -> Uuid {
    let response = request
        .post("/api/template-library")
        .add_header("Authorization", format!("Bearer {key}"))
        .json(&json!({ "name": "library-note", "definition": definition }))
        .await;
    response.assert_status(StatusCode::CREATED);
    serde_json::from_value(response.json::<serde_json::Value>()["id"].clone())
        .expect("created template ID")
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
async fn a_schema_with_no_origin_is_never_reported_or_mergeable() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs {
            key_scope: ApiKeyScope::Schema,
            ..Default::default()
        }).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&note_definition())
            .await;
        assert_eq!(
            create.status_code(),
            StatusCode::CREATED,
            "response: {:?}",
            create.text()
        );
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let changes = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(changes.status_code(), StatusCode::OK);
        assert!(
            changes.json::<Vec<serde_json::Value>>().is_empty(),
            "a schema with no origin template must never be reported"
        );

        let preview = request
            .get(&format!("/api/schemas/{schema_id}/merge-preview"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            preview.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "response: {:?}",
            preview.text()
        );
        let body: serde_json::Value = preview.json();
        assert_eq!(body["error"]["code"], "validation_failed");
        assert_eq!(
            body["error"]["message"],
            format!("schema '{schema_id}' does not follow a template")
        );
        assert_eq!(
            body["error"]["hint"],
            "only a schema created from a template library entry can be merged; create one with POST /api/schemas and a template_id"
        );
    })
    .await;
}

/// Separate workspaces can merge concurrently, including on single-tenant SQLite.
#[tokio::test]
async fn concurrent_rest_merges_commit_for_separate_workspaces() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let mut inputs = Vec::new();
        let tenant = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let template = create_template(&request, &tenant.api_key, note_definition()).await;
        for _ in 0..2 {
            let txn = ctx.db.begin().await.unwrap();
            let workspace = yorishiro::models::tenancy::create_workspace(
                &txn,
                tenant.tenant_id,
                &format!("merge-{}", Uuid::new_v4()),
                None,
                None,
            )
            .await
            .unwrap();
            txn.commit().await.unwrap();
            let key = super::fixtures::issue_api_key(
                &ctx,
                workspace.id,
                tenant.owner_id,
                ApiKeyScope::Schema,
                false,
            )
            .await;
            let response = request
                .post("/api/schemas")
                .add_header("Authorization", format!("Bearer {key}"))
                .json(&json!({"template_id":template.to_string()}))
                .await;
            response.assert_status(StatusCode::CREATED);
            let id = response.json::<serde_json::Value>()["schema"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            inputs.push((key, id));
        }
        let barrier = tokio::sync::Barrier::new(2);
        let merge = |(key, id): (String, String)| {
            let request = &request;
            let barrier = &barrier;
            async move {
                barrier.wait().await;
                let response = request
                    .post(&format!("/api/schemas/{id}/merge"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .await;
                response.assert_status(StatusCode::CREATED);
                let body = response.json::<serde_json::Value>();
                let created = body["schema"]["id"].as_str().unwrap();
                let persisted = request
                    .get(&format!("/api/schemas/{created}"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .await;
                persisted.assert_status(StatusCode::OK);
                assert_eq!(body["schema"]["version"], 2);
            }
        };
        tokio::join!(merge(inputs.pop().unwrap()), merge(inputs.pop().unwrap()));
    })
    .await;
}

#[tokio::test]
async fn upstream_changes_preview_and_merge_round_trip() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs {
            key_scope: ApiKeyScope::Schema,
            ..Default::default()
        }).await;
        let template = create_template(&request, &setup.api_key, note_definition()).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({ "template_id": template.to_string() }))
            .await;
        assert_eq!(
            create.status_code(),
            StatusCode::CREATED,
            "response: {:?}",
            create.text()
        );

        // Not yet reported: the template has not moved since the copy was taken.
        let before = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert!(before.json::<Vec<serde_json::Value>>().is_empty());

        // Add a field to the workspace's own copy, so the merge has something local to keep.
        // This is a second version of the same name (create_schema archives the first and installs this as the new active row), so it has its own id: the origin/merge chain always acts on the currently active version, which is this one from here on.
        let edit_local = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
        let updated_definition: yorishiro::models::schema_schemas::metaschema::MetaSchemaDefinition =
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
        for _ in 0..2 {
            request
                .put(&format!("/api/template-library/{template}"))
                .add_header("Authorization", format!("Bearer {}", setup.api_key))
                .json(&json!({ "definition": updated_definition }))
                .await
                .assert_status(StatusCode::OK);
        }

        // A local version created after publication inherits the pending state instead of acknowledging it.
        let local_after_publication = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            preview.status_code(),
            StatusCode::OK,
            "response: {:?}",
            preview.text()
        );
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            merge.status_code(),
            StatusCode::CREATED,
            "response: {:?}",
            merge.text()
        );
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
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert!(acknowledged.json::<Vec<serde_json::Value>>().is_empty());
    })
    .await;
}

/// A publication racing with merge cannot be acknowledged by the merge that read the older revision.
#[tokio::test]
async fn publication_waits_for_merge_revision_lock_and_remains_pending() {
    // SQLite has no advisory lock to block publication behind the merge revision lock.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let request = std::sync::Arc::new(request);
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let template = create_template(&request, &setup.api_key, note_definition()).await;
        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({ "template_id": template.to_string() }))
            .await;
        assert_eq!(create.status_code(), StatusCode::CREATED);
        let schema_id: Uuid = create.json::<serde_json::Value>()["schema"]["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        request
            .put(&format!("/api/template-library/{template}"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({ "definition": {
                "name": "library-note",
                "entity_types": { "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "category": { "type": "string" }
                } } }
            } }))
            .await
            .assert_status(StatusCode::OK);

        // Hold only this workspace's schema lock so merge stops after taking the origin lock.
        let blocker = ctx.db.begin().await.expect("begin schema lock blocker");
        yorishiro::models::schema_schemas::lock_version(
            &blocker, setup.workspace_id, "library-note",
        ).await.expect("hold schema version lock");
        let merge_request = request.clone();
        let merge_key = setup.api_key.clone();
        let merge = async move {
            merge_request.post(&format!("/api/schemas/{schema_id}/merge"))
                .add_header("Authorization", format!("Bearer {merge_key}"))
                .await
        };

        // Observe the real lock waits, not a test hook or a fixed delay.
        let wait_for_lock = |key: String| {
            let db = ctx.db.clone();
            async move {
                use sea_orm::{FromQueryResult, Statement};
                #[derive(FromQueryResult)]
                struct LockWait { waiting: bool }
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        let waiting = LockWait::find_by_statement(Statement::from_sql_and_values(
                            sea_orm::DatabaseBackend::Postgres,
                            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS waiting",
                            [key.clone().into()],
                        )).one(&db).await.expect("observe revision lock wait")
                            .expect("lock observation").waiting;
                        if waiting { break; }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                }).await.expect("request waits on the target advisory lock");
            }
        };
        let publish_after_merge_waits = async {
        wait_for_lock(format!("{}:library-note", setup.workspace_id)).await;
        let update_request = request.clone();
        let update_key = setup.api_key.clone();
        let update = async move {
            update_request.put(&format!("/api/template-library/{template}"))
                .add_header("Authorization", format!("Bearer {update_key}"))
                .json(&json!({ "definition": {
                    "name": "library-note",
                    "entity_types": { "note": { "fields": {
                        "title": { "type": "string", "required": true },
                        "category": { "type": "string" },
                        "status": { "type": "string" }
                    } } }
                } }))
                .await
        };
        let release = async {
            wait_for_lock(format!("template-origin:{template}")).await;
            blocker.rollback().await.expect("release schema lock");
        };
        let (published, ()) = tokio::join!(update, release);
        published.assert_status(StatusCode::OK);
        };
        let (merged, ()) = tokio::join!(merge, publish_after_merge_waits);
        merged.assert_status(StatusCode::CREATED);
        assert_eq!(merged.json::<serde_json::Value>()["schema"]["version"], 2);

        let changes = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
async fn merging_a_conflicting_field_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let base_def = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "priority": { "type": "string" } } }
            }
        });
        let template = create_template(&request, &setup.api_key, base_def.clone()).await;

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({ "template_id": template.to_string() }))
            .await;
        create.assert_status(StatusCode::CREATED);

        let local = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({
                "name": "library-note",
                "entity_types": {
                    "note": { "fields": { "priority": { "type": "boolean" } } }
                }
            }))
            .await;
        local.assert_status(StatusCode::CREATED);
        let schema_id: Uuid =
            serde_json::from_value(local.json::<serde_json::Value>()["schema"]["id"].clone())
                .expect("active local schema ID");

        request
            .put(&format!("/api/template-library/{template}"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .json(&json!({ "definition": {
                        "name": "library-note",
                        "entity_types": {
                            "note": { "fields": { "priority": { "type": "integer" } } }
                        }
            } }))
            .await
            .assert_status(StatusCode::OK);

        let merge = request
            .post(&format!("/api/schemas/{schema_id}/merge"))
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        assert_eq!(
            merge.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "response: {:?}",
            merge.text()
        );
        assert!(
            merge.json::<serde_json::Value>()["error"]["message"]
                .as_str()
                .expect("merge error message")
                .contains("conflict")
        );
        let pending = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
            .await;
        let pending_body: Vec<serde_json::Value> = pending.json();
        assert_eq!(pending_body.len(), 1);
        assert_eq!(pending_body[0]["pending_notification"], true);
        assert_eq!(pending_body[0]["summary"]["conflict"], 1);
    })
    .await;
}
