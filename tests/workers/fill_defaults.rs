use sea_orm::{EntityTrait, TransactionTrait};
use uuid::Uuid;

use crate::requests::boot_request;
use crate::requests::jobs::{finish_job, finish_via_api};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one migration history verifies fill and undo before testing queued credential and IAM changes through REST and MCP without modifying the entity"
)]
async fn rest_fill_defaults_waits_for_a_loco_worker() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys::ApiKeyScope, entity_entities, schema_schemas};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant { tenant_id: tenant, workspace_id: workspace, api_key: key, owner_id,  } = crate::requests::fixtures::TestTenant::create(&request, &ctx, crate::requests::fixtures::TenantArgs { key_scope: ApiKeyScope::Migration, ..Default::default() },
        ).await;
        let iam_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant, owner_id).await;
        let mut entity_id = None;
        for (version, fields) in [json!({"title": {"type": "string", "required": true}}),
            json!({"title": {"type": "string", "required": true}, "rank": {"type": "integer", "required": true, "default": 7}})]
            .into_iter().enumerate()
        {
            schema_schemas::create_schema(&ctx.db, tenant, workspace,
                serde_json::from_value(json!({"name": "note", "entity_types": {"note": {"fields": fields}}})).unwrap(),
                None, None).await.unwrap();
            if version == 0 {
                entity_id = Some(entity_entities::create(&ctx.db, workspace, entity_entities::CreateEntityInput {
                    schema_name: "note".into(), entity_type: "note".into(), data: json!({"title": "before"}),
                }, None).await.unwrap().id);
            }
        }
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&json!({"schema_name": "note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED, "{}", response.text());
        let id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        let entity_id = entity_id.unwrap();
        let before = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(before.data, json!({"title": "before"}));
        assert_eq!(entity_entities::migration_dry_run(&ctx.db, workspace, "note").await.unwrap().needs_values, 1);
        finish_via_api(&request, &ctx, id, &key, &format!("/api/migration-jobs/{id}")).await;
        let after = entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap();
        assert_eq!(after.data, json!({"title": "before", "rank": 7}));
        assert_eq!(after.schema_version, before.schema_version);
        assert!(entity_entities::drift(&ctx.db, workspace, entity_id).await.unwrap().missing_fields.is_empty());
        let report = entity_entities::migration_dry_run(&ctx.db, workspace, "note").await.unwrap();
        assert_eq!(report.needs_values, 0);
        assert_eq!(report.behind_but_valid, 1);
        let response = request.post(&format!("/api/migration-jobs/{id}/undo"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let undo_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        finish_via_api(&request, &ctx, undo_id, &key, &format!("/api/migration-jobs/{undo_id}")).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, before.data);
        assert_eq!(entity_entities::migration_dry_run(&ctx.db, workspace, "note").await.unwrap().needs_values, 1);
        assert_eq!(entity_entities::drift(&ctx.db, workspace, entity_id).await.unwrap().missing_fields[0].name, "rank");

        let revoked = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db, workspace, ApiKeyScope::Migration, Some(owner_id), false,
        ).await.unwrap();
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", revoked.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let denied_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        yorishiro::models::api_keys::Entity::revoke(&ctx.db, revoked.id).await.unwrap();
        finish_job(&ctx, denied_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{denied_id}")))).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, before.data);

        let txn = ctx.db.begin().await.unwrap();
        // A policy-free member: an admin carries the built-in wildcard policy and would survive the attachment removal.
        let member = yorishiro::models::tenancy::register_member(
            &txn, tenant, &crate::requests::fixtures::unique_email("queued-member"),
            "hunter2-hunter2", None).await.unwrap();
        txn.commit().await.unwrap();
        let document = json!({"statements":[{"effect":"allow","actions":["migration_fill_defaults"],
            "resources":[{"type":"workspace","tenant_id":tenant,"workspace_id":workspace}]}]});
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {iam_key}"))
            .json(&json!({"name":"queued-fill","document":document})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/iam/users/{}/policies/{policy_id}", member.id);
        request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let attributed = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db, workspace, ApiKeyScope::Migration, Some(member.id), false,
        ).await.unwrap();
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", attributed.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let downgraded_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        finish_job(&ctx, downgraded_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{downgraded_id}")))).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, before.data);
        assert!(yorishiro::models::api_keys::Entity::find_by_id(attributed.id).one(&ctx.db).await.unwrap().is_some());
        request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let pending = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", attributed.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        pending.assert_status(StatusCode::ACCEPTED);
        let pending_id = pending.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        let mut deny = document.clone();
        deny["statements"][0]["effect"] = json!("deny");
        request.put(&format!("/api/iam/policies/{policy_id}"))
            .add_header("Authorization", format!("Bearer {iam_key}"))
            .json(&json!({"name":"queued-fill","document":deny})).await.assert_status_ok();
        finish_job(&ctx, pending_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{pending_id}")))).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, before.data);
        request.put(&format!("/api/iam/policies/{policy_id}"))
            .add_header("Authorization", format!("Bearer {iam_key}"))
            .json(&json!({"name":"queued-fill","document":document})).await.assert_status_ok();
        let failed = request.get(&format!("/api/migration-jobs/{downgraded_id}"))
            .add_header("Authorization", format!("Bearer {key}")).await;
        assert_eq!(failed.status_code(), StatusCode::OK);
        assert_eq!(failed.json::<serde_json::Value>()["status"], "failed");

        let removed = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db, workspace, ApiKeyScope::Migration, Some(member.id), false,
        ).await.unwrap();
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", removed.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let removed_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        yorishiro::models::tenant_memberships::remove_member(&ctx.db, tenant, member.id).await.unwrap();
        yorishiro::models::tenant_memberships::add_member(&ctx.db, tenant, member.id).await.unwrap();
        for id in [attributed.id, removed.id] {
            assert!(yorishiro::models::api_keys::Entity::find_by_id(id).one(&ctx.db).await.unwrap().expect("revoked grant history retained").revoked_at.is_some());
        }
        let rejected = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", removed.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        assert_eq!(rejected.status_code(), StatusCode::UNAUTHORIZED);
        finish_job(&ctx, removed_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{removed_id}")))).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, before.data);
        request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);

        let attributed = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db, workspace, ApiKeyScope::Migration, Some(member.id), false,
        ).await.unwrap();
        let response = request.post("/api/migration-jobs/fill-defaults")
            .add_header("Authorization", format!("Bearer {}", attributed.plaintext))
            .json(&json!({"schema_name":"note"})).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let filled_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        finish_via_api(&request, &ctx, filled_id, &key, &format!("/api/migration-jobs/{filled_id}")).await;
        let revoked = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db, workspace, ApiKeyScope::Migration, Some(owner_id), false,
        ).await.unwrap();
        let response = request.post(&format!("/api/migration-jobs/{filled_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", revoked.plaintext)).await;
        assert_eq!(response.status_code(), StatusCode::ACCEPTED);
        let denied_id = response.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap();
        yorishiro::models::api_keys::Entity::revoke(&ctx.db, revoked.id).await.unwrap();
        finish_job(&ctx, denied_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{denied_id}")))).await;
        assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, after.data);
        let undo_document = json!({"statements":[{"effect":"allow","actions":["migration_undo"],
            "resources":[{"type":"workspace","tenant_id":tenant,"workspace_id":workspace}]}]});
        request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let session = crate::controllers::mcp::initialize(&request).await;
        for explicit_deny in [false, true] {
            request.put(&format!("/api/iam/policies/{policy_id}"))
                .add_header("Authorization", format!("Bearer {iam_key}"))
                .json(&json!({"name":"queued-fill","document":undo_document})).await.assert_status_ok();
            request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let undo_id: Uuid = if explicit_deny {
                let queued = crate::controllers::mcp::call_tool(&request, &session, &attributed.plaintext,
                    "undo_migration_job", json!({"job_id":filled_id})).await;
                assert_ne!(queued["result"]["isError"], true, "{queued}");
                let queued: serde_json::Value = serde_json::from_str(queued["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                queued["job_id"].as_str().unwrap().parse().unwrap()
            } else {
                let queued = request.post(&format!("/api/migration-jobs/{filled_id}/undo"))
                    .add_header("Authorization", format!("Bearer {}", attributed.plaintext)).await;
                queued.assert_status(StatusCode::ACCEPTED);
                queued.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap()
            };
            request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            if explicit_deny {
                let mut deny = undo_document.clone();
                deny["statements"][0]["effect"] = json!("deny");
                request.put(&format!("/api/iam/policies/{policy_id}"))
                    .add_header("Authorization", format!("Bearer {iam_key}"))
                    .json(&json!({"name":"queued-fill","document":deny})).await.assert_status_ok();
                request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
            finish_job(&ctx, undo_id, "failed", Some((&request, &key, &format!("/api/migration-jobs/{undo_id}")))).await;
            assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, after.data);
        }
        for explicit_deny in [false, true] {
            let document = json!({"statements":[{"effect":"allow","actions":["workspace_reindex"],
                "resources":[{"type":"workspace","tenant_id":tenant,"workspace_id":workspace}]}]});
            request.put(&format!("/api/iam/policies/{policy_id}"))
                .add_header("Authorization", format!("Bearer {iam_key}"))
                .json(&json!({"name":"queued-fill","document":document})).await.assert_status_ok();
            let id: Uuid = if explicit_deny {
                let queued = crate::controllers::mcp::call_tool(&request, &session, &attributed.plaintext,
                    "reindex_workspace", json!({})).await;
                assert_ne!(queued["result"]["isError"], true, "{queued}");
                let queued: serde_json::Value = serde_json::from_str(queued["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                queued["job_id"].as_str().unwrap().parse().unwrap()
            } else {
                let queued = request.post("/api/migration-jobs/reindex")
                    .add_header("Authorization", format!("Bearer {}", attributed.plaintext)).await;
                queued.assert_status_ok();
                queued.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap()
            };
            let before_workspace = yorishiro::models::workspace_workspaces::Entity::find_by_id(workspace).one(&ctx.db).await.unwrap().unwrap();
            if explicit_deny {
                let mut deny = document;
                deny["statements"][0]["effect"] = json!("deny");
                request.put(&format!("/api/iam/policies/{policy_id}"))
                    .add_header("Authorization", format!("Bearer {iam_key}"))
                    .json(&json!({"name":"queued-fill","document":deny})).await.assert_status_ok();
            } else {
                request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
            finish_job(&ctx, id, "failed", None).await;
            let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
            assert!(format!("{lifecycle:?}").contains("IAM policy does not permit"), "{lifecycle:?}");
            let after_workspace = yorishiro::models::workspace_workspaces::Entity::find_by_id(workspace).one(&ctx.db).await.unwrap().unwrap();
            assert_eq!(before_workspace.embedding_revision, after_workspace.embedding_revision);
            assert_eq!(before_workspace.embedding_reindexing, after_workspace.embedding_reindexing);
            assert_eq!(entity_entities::get(&ctx.db, workspace, entity_id).await.unwrap().data, after.data);
            if !explicit_deny {
                request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
        }
    }).await;
}
