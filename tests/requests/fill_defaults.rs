use std::time::Duration;

use futures::FutureExt;
use loco_rs::app::{AppContext, Hooks};
use sea_orm::{ConnectionTrait, EntityTrait, TransactionTrait};
use uuid::Uuid;

pub(crate) use super::boot_request_isolated as boot_request;

#[cfg(feature = "enterprise")]
pub(crate) async fn finish(ctx: &AppContext, id: Uuid) {
    finish_with_status(ctx, id, "completed").await;
}

#[cfg(feature = "enterprise")]
pub(crate) async fn finish_with_status(ctx: &AppContext, id: Uuid, expected: &str) {
    finish_job(ctx, id, expected, None).await;
}

pub(crate) async fn finish_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "completed", Some((request, key, status_path))).await;
}

#[cfg(feature = "enterprise")]
pub(crate) async fn fail_via_api(
    request: &loco_rs::TestServer,
    ctx: &AppContext,
    id: Uuid,
    key: &str,
    status_path: &str,
) {
    finish_job(ctx, id, "failed", Some((request, key, status_path))).await;
}

async fn finish_job(
    ctx: &AppContext,
    id: Uuid,
    expected: &str,
    status_api: Option<(&loco_rs::TestServer, &str, &str)>,
) {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    // Capture relay failures from spawned tasks; queue state alone cannot explain a missing publish.
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_test_writer()
                .with_ansi(false),
        )
        .with(
            tracing_subscriber::filter::Targets::new()
                .with_target("yorishiro::workers::dispatch", tracing::Level::WARN)
                .with_target("yorishiro::initializers::queue", tracing::Level::ERROR),
        )
        .try_init();
    let outbox = yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let tag = outbox.tags.unwrap()[0].as_str().unwrap().to_owned();
    let mut worker_ctx = ctx.clone();
    let queue = super::query_worker::queue_for_tag(&mut worker_ctx, tag.clone()).await;
    let running = queue.clone();
    let worker = tokio::spawn(async move { running.run(vec![tag]).await });
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let status = if let Some((request, key, path)) = status_api {
                    let response = request
                        .get(path)
                        .add_header("Authorization", format!("Bearer {key}"))
                        .await;
                    response.assert_status(axum::http::StatusCode::OK);
                    response.json::<serde_json::Value>()["status"]
                        .as_str()
                        .expect("job status")
                        .to_owned()
                } else {
                    yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
                        .one(&ctx.db)
                        .await
                        .unwrap()
                        .unwrap()
                        .status
                };
                if matches!(status.as_str(), "completed" | "failed") {
                    assert_eq!(status, expected, "worker outcome");
                    let directory = tempfile::tempdir().expect("queue result directory");
                    let path = queue.dump(directory.path(), None, None).await.unwrap();
                    let jobs: Vec<loco_rs::bgworker::Job> =
                        serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                    let expected_status = if expected == "failed" {
                        loco_rs::bgworker::JobStatus::Failed
                    } else {
                        loco_rs::bgworker::JobStatus::Completed
                    };
                    if jobs.iter().any(|job| {
                        job.data["lifecycle_id"] == id.to_string() && job.status == expected_status
                    }) {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }))
        .catch_unwind()
        .await;
    if matches!(&result, Ok(Err(_))) {
        let lifecycle = yorishiro::models::queue_job_lifecycles::Entity::find_by_id(id)
            .one(&ctx.db)
            .await;
        let dispatch =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(id)
                .one(&ctx.db)
                .await;
        eprintln!(
            "worker timeout: lifecycle={:?}, dispatch={:?}",
            lifecycle.map(|row| row.map(|row| (row.status, row.attempt, row.provider_job_id))),
            dispatch.map(|row| row.map(|row| (
                row.published_at,
                row.last_attempt_at,
                row.claim_until
            ))),
        );
        let directory = tempfile::tempdir().expect("queue diagnostic directory");
        match queue.dump(directory.path(), None, None).await {
            Ok(path) => {
                let jobs: serde_yaml::Value = serde_yaml::from_str(
                    &std::fs::read_to_string(path).expect("read queue diagnostics"),
                )
                .expect("decode queue diagnostics");
                if let Some(jobs) = jobs.as_sequence() {
                    for job in jobs {
                        eprintln!(
                            "worker timeout queue job: id={:?}, name={:?}, status={:?}",
                            job.get("id"),
                            job.get("name"),
                            job.get("status")
                        );
                    }
                }
            }
            Err(error) => eprintln!("worker timeout queue inspection failed: {error}"),
        }
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
            let activity = ctx.db.query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT COALESCE(jsonb_agg(jsonb_build_object('pid', pid, 'state', state, 'wait_type', wait_event_type, 'wait', wait_event, 'blockers', pg_blocking_pids(pid))), '[]'::jsonb) AS activity FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid()",
            )).await.map(|row| row.map(|row| row.try_get::<serde_json::Value>("", "activity")));
            eprintln!("worker timeout database activity: {activity:?}");
        }
    }
    yorishiro::App::on_shutdown(&worker_ctx).await;
    let shutdown = queue.shutdown();
    let joined = tokio::time::timeout(Duration::from_secs(10), worker).await;
    let closed = queue.close().await;
    match result {
        Ok(outcome) => outcome.expect("migration must complete on the configured queue provider"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    shutdown.expect("shutdown worker queue");
    joined
        .expect("drain worker")
        .expect("join worker")
        .expect("run worker");
    closed.expect("close drained worker queue");
}

#[tokio::test]
async fn rest_fill_defaults_waits_for_a_loco_worker() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys::ApiKeyScope, entity_entities, schema_schemas};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let super::fixtures::TestTenant { tenant_id: tenant, workspace_id: workspace, api_key: key, owner_id, .. } = super::fixtures::TestTenant::create(&request, &ctx, super::fixtures::TenantArgs { key_scope: ApiKeyScope::Migration, ..Default::default() },
        ).await;
        let iam_key = super::fixtures::issue_tenant_key(&ctx, tenant, owner_id).await;
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
        let member = yorishiro::models::tenancy::register_member(
            &txn, tenant, &super::fixtures::unique_email("queued-member"),
            "hunter2-hunter2", None,
            yorishiro::models::tenant_memberships::MembershipRole::Admin,
        ).await.unwrap();
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
        yorishiro::models::tenant_memberships::add_member(&ctx.db, tenant, member.id,
            yorishiro::models::tenant_memberships::MembershipRole::Admin).await.unwrap();
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
        let initialized = request.post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"undo-iam","version":"1"}
            }})).await;
        initialized.assert_status_ok();
        let session = initialized.headers()["mcp-session-id"].to_str().unwrap().to_owned();
        for explicit_deny in [false, true] {
            request.put(&format!("/api/iam/policies/{policy_id}"))
                .add_header("Authorization", format!("Bearer {iam_key}"))
                .json(&json!({"name":"queued-fill","document":undo_document})).await.assert_status_ok();
            request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let undo_id: Uuid = if explicit_deny {
                let queued = super::iam::iam_mcp_call(&request, &session, &attributed.plaintext,
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
                let queued = super::iam::iam_mcp_call(&request, &session, &attributed.plaintext,
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

#[tokio::test]
async fn queued_import_rechecks_revoked_credentials_before_writing() {
    queued_import_rechecks_inactive_credentials(false).await;
}

#[tokio::test]
async fn queued_import_rechecks_expired_credentials_before_writing() {
    queued_import_rechecks_inactive_credentials(true).await;
}

async fn queued_import_rechecks_inactive_credentials(expired: bool) {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys, schema_schemas};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = super::fixtures::TestTenant::create(
            &request,
            &ctx,
            super::fixtures::TenantArgs {
                key_scope: api_keys::ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Schema,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap();
        let jsonl = json!({"kind":"schema", "record":{
            "id": Uuid::new_v4(), "tenant_id":tenant.tenant_id, "workspace_id":tenant.workspace_id,
            "name":"revoked-import", "version":1,
            "definition":{"name":"revoked-import","entity_types":{},"relation_types":{}},
            "status":"active", "origin_template_id":null, "origin_status":"detached",
            "origin_snapshot":null, "origin_updated_at":null, "created_at":chrono::Utc::now()
        }})
        .to_string();
        let response = request
            .post("/api/import.jsonl")
            .add_header("Authorization", format!("Bearer {}", key.plaintext))
            .text(jsonl)
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::ACCEPTED,
            "{}",
            response.text()
        );
        let id = response.json::<serde_json::Value>()["job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        if expired {
            use sea_orm::{ActiveModelTrait, ActiveValue::Set};
            api_keys::ActiveModel {
                id: Set(key.id),
                expires_at: Set(Some(
                    (chrono::Utc::now() - chrono::Duration::seconds(1)).fixed_offset(),
                )),
                ..Default::default()
            }
            .update(&ctx.db)
            .await
            .unwrap();
        } else {
            api_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        }
        finish_job(
            &ctx,
            id,
            "failed",
            Some((&request, &tenant.api_key, &format!("/api/import-jobs/{id}"))),
        )
        .await;
        assert!(
            schema_schemas::get_active_schema(&ctx.db, tenant.workspace_id, "revoked-import")
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn queued_import_rechecks_iam_policies_before_writing() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{
        api_keys, schema_schemas, tenancy,
        tenant_memberships::MembershipRole,
        workspace_memberships::{Model, WorkspaceRole},
    };

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = super::fixtures::TestTenant::create(
            &request,
            &ctx,
            super::fixtures::TenantArgs::default(),
        )
        .await;
        let txn = ctx.db.begin().await.unwrap();
        let member = tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &super::fixtures::unique_email("queued-workspace"),
            "hunter2-hunter2",
            None,
            MembershipRole::Admin,
        )
        .await
        .unwrap();
        Model::grant(&txn, tenant.workspace_id, member.id, WorkspaceRole::Admin)
            .await
            .unwrap();
        txn.commit().await.unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Schema,
            Some(member.id),
            false,
        )
        .await
        .unwrap();
        let jsonl = json!({"kind":"schema", "record":{
            "id":Uuid::new_v4(), "tenant_id":tenant.tenant_id, "workspace_id":tenant.workspace_id,
            "name":"grant-revoked-import", "version":1,
            "definition":{"name":"grant-revoked-import","entity_types":{},"relation_types":{}},
            "status":"active", "origin_template_id":null,"origin_status":"detached",
            "origin_snapshot":null,"origin_updated_at":null,"created_at":chrono::Utc::now()
        }})
        .to_string();
        let iam_key = super::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let document = json!({"statements":[{"effect":"allow","actions":["import_create"],
            "resources":[{"type":"workspace","tenant_id":tenant.tenant_id,"workspace_id":tenant.workspace_id}]}]});
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {iam_key}"))
            .json(&json!({"name":"queued-import","document":document})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/iam/users/{}/policies/{policy_id}", member.id);
        let initialized = request.post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"queued-import","version":"1"}
            }})).await;
        initialized.assert_status_ok();
        let session = initialized.headers()["mcp-session-id"].to_str().unwrap().to_owned();
        for explicit_deny in [false, true] {
            request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let id: Uuid = if explicit_deny {
                let queued = super::iam::iam_mcp_call(&request, &session, &key.plaintext,
                    "import_jsonl", json!({"jsonl":jsonl})).await;
                assert_ne!(queued["result"]["isError"], true, "{queued}");
                let queued: serde_json::Value = serde_json::from_str(queued["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                queued["job_id"].as_str().unwrap().parse().unwrap()
            } else {
                let queued = request.post("/api/import.jsonl")
                    .add_header("Authorization", format!("Bearer {}", key.plaintext))
                    .text(jsonl.clone()).await;
                queued.assert_status(StatusCode::ACCEPTED);
                queued.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap()
            };
            if explicit_deny {
                let mut deny = document.clone();
                deny["statements"][0]["effect"] = json!("deny");
                request.put(&format!("/api/iam/policies/{policy_id}"))
                    .add_header("Authorization", format!("Bearer {iam_key}"))
                    .json(&json!({"name":"queued-import","document":deny})).await.assert_status_ok();
            }
            request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            if explicit_deny {
                request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
            finish_job(&ctx, id, "failed", Some((&request, &tenant.api_key, &format!("/api/import-jobs/{id}")))).await;
            assert!(schema_schemas::get_active_schema(&ctx.db, tenant.workspace_id, "grant-revoked-import").await.is_err());
        }
    }).await;
}
