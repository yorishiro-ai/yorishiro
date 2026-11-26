use crate::requests::{boot_request, boot_request_with_timezone};
use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
use sea_orm::{EntityTrait, TransactionTrait};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::fixtures::{TenantArgs, TestTenant, issue_access_key};

struct Setup {
    owner_id: Uuid,
    tenant_id: Uuid,
    workspace_id: Uuid,
    migration_key: String,
    audit_key: String,
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
async fn setup(request: &loco_rs::TestServer, ctx: &loco_rs::app::AppContext) -> Setup {
    let TestTenant {
        tenant_id,
        workspace_id,
        owner_id,
        api_key: migration_key,
    } = TestTenant::create(request, ctx, TenantArgs::default()).await;
    let audit_key = issue_access_key(ctx, workspace_id, owner_id).await;

    Setup {
        owner_id,
        tenant_id,
        workspace_id,
        migration_key,
        audit_key,
    }
}

#[tokio::test]
async fn audit_reads_require_iam_and_preserve_the_configured_timezone() {
    boot_request_with_timezone::<App, _, _>("Asia/Tokyo", |request, ctx| async move {
        let timezone = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap()
            .timezone
            .parse::<chrono_tz::Tz>()
            .unwrap();
        assert_ne!(
            timezone,
            chrono_tz::UTC,
            "test must boot with a non-default zone"
        );
        let setup = setup(&request, &ctx).await;
        let txn = ctx.db.begin().await.unwrap();
        // A policy-free member: an admin carries the built-in wildcard policy and would pass the pre-attachment read.
        let member = yorishiro::models::tenancy::register_member(
            &txn, setup.tenant_id, &crate::requests::fixtures::unique_email("audit-reader"),
            "audit-reader-password", None).await.unwrap();
        txn.commit().await.unwrap();
        let delegated = issue_access_key(&ctx, setup.workspace_id, member.id).await;
        let root_key = crate::requests::fixtures::issue_tenant_key(&ctx, setup.tenant_id, setup.owner_id).await;
        let document = serde_json::json!({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["audit:ListWorkspaceAuditEvents"],
            "Resource":[format!("yrn:workspace:{}:{}", setup.tenant_id, setup.workspace_id)]}]});
        let policy = request.post("/api/policies")
            .add_header("Authorization", format!("Bearer {root_key}"))
            .json(&serde_json::json!({"name":"audit-reader","document":document})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/users/{}/policies/{policy_id}", member.id);
        request.get("/api/audit-log").add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status(StatusCode::FORBIDDEN);
        request.put(&attachment).add_header("Authorization", format!("Bearer {root_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        request.get("/api/audit-log").add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status_ok();
        let mut denied = document.clone();
        denied["Statement"][0]["Effect"] = serde_json::json!("Deny");
        crate::requests::fixtures::replace_policy_document(&request, &format!("Bearer {root_key}"), &policy_id, &denied).await;
        request.get("/api/audit-log").add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status(StatusCode::FORBIDDEN);

        request
            .post("/api/migration-jobs/reindex")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await
            .assert_status_ok();

        let log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", setup.audit_key))
            .await;
        assert_eq!(
            log.status_code(),
            StatusCode::OK,
            "response: {:?}",
            log.text()
        );
        let body: Vec<serde_json::Value> = log.json();
        assert_eq!(body.len(), 1, "body: {body:?}");
        assert_eq!(body[0]["action"], "reindex_embeddings");
        assert_eq!(body[0]["detail"]["workspace_id"], setup.workspace_id.to_string());
        for event in &body {
            let created_at =
                chrono::DateTime::parse_from_rfc3339(event["created_at"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                created_at.offset().local_minus_utc(),
                timezone
                    .offset_from_utc_datetime(&created_at.naive_utc())
                    .fix()
                    .local_minus_utc()
            );
        }
    })
    .await;
}

/// `undo_migration_job` is recorded on the same transaction as the undo itself.
#[tokio::test]
async fn undo_migration_job_is_recorded() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&request, &ctx).await;

        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .expect("parse definition");
        yorishiro::models::schema_schemas::create_schema(
            &ctx.db,
            setup.tenant_id,
            setup.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .expect("create schema");
        let entity = yorishiro::models::entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            yorishiro::models::entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let job_id = Uuid::new_v4();
        yorishiro::models::entity_entities::snapshot(
            &ctx.db,
            setup.workspace_id,
            entity.id,
            job_id,
        )
        .await
        .expect("snapshot entity");
        yorishiro::models::entity_entities::update(
            &ctx.db,
            setup.workspace_id,
            yorishiro::models::entity_entities::UpdateEntityInput {
                id: entity.id,
                data: serde_json::json!({ "title": "overwritten" }),
                updated_by: None,
                expected_updated_at: None,
            },
        )
        .await
        .expect("overwrite entity");

        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}:undo"))
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::ACCEPTED,
            "response: {:?}",
            undo.text()
        );
        let queued_id: Uuid = undo.json::<serde_json::Value>()["migration_job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        crate::requests::jobs::finish_via_api(
            &request,
            &ctx,
            queued_id,
            &setup.migration_key,
            &format!("/api/migration-jobs/{queued_id}"),
        )
        .await;

        let log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", setup.audit_key))
            .await;
        let body: Vec<serde_json::Value> = log.json();
        assert_eq!(body.len(), 1, "body: {body:?}");
        assert_eq!(body[0]["action"], "undo_migration_job");
        assert_eq!(body[0]["detail"]["job_id"], queued_id.to_string());
        assert_eq!(body[0]["detail"]["snapshot_job_id"], job_id.to_string());
        assert_eq!(body[0]["detail"]["restored"], 1);
    })
    .await;
}

/// Reindex intentionally records workspace identity rather than the queue job ID.
/// The audit row and dispatch outbox are written in the caller transaction, which commits before the relay publishes.
/// The response exposes the durable job ID, not a provider publication ID.
#[tokio::test]
async fn reindex_audit_detail_is_workspace_id_and_is_recorded_before_relay_publication() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&request, &ctx).await;
        let response = request
            .post("/api/migration-jobs/reindex")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::OK,
            "response: {:?}",
            response.text()
        );
        let dispatch_id: Uuid = response.json::<serde_json::Value>()["migration_job_id"]
            .as_str()
            .expect("durable job ID")
            .parse()
            .unwrap();
        let expected = serde_json::json!({ "workspace_id": setup.workspace_id });
        // Atomic enqueue observation: the relay must see both committed records.
        let committed_outbox =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(
                dispatch_id,
            )
            .one(&ctx.db)
            .await
            .expect("relay reads committed outbox")
            .expect("outbox committed with audit row");
        assert_eq!(committed_outbox.lifecycle_id, Some(dispatch_id));
        assert!(committed_outbox.published_at.is_none());
        let committed_audit = yorishiro::models::access_key_audit_log::list_for_workspace(
            &ctx.db,
            setup.workspace_id,
            yorishiro::models::pagination::ListParams::default(),
        )
        .await
        .expect("relay reads committed audit rows");
        assert_eq!(committed_audit.len(), 1, "audit row committed with outbox");
        assert_eq!(committed_audit[0].detail, expected);
        let log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", setup.audit_key))
            .await;
        assert_eq!(
            log.status_code(),
            StatusCode::OK,
            "response: {:?}",
            log.text()
        );
        let body: Vec<serde_json::Value> = log.json();
        assert_eq!(body.len(), 1, "body: {body:?}");
        assert_eq!(body[0]["action"], "reindex_embeddings");
        assert_eq!(body[0]["detail"], expected);
        assert!(body[0]["detail"].get("job_id").is_none());
    })
    .await;
}

/// RLS isolation, exercised through `Authorized` (the RLS-scoped path a real request takes), not
/// `ctx.db` (the migration-role connection, which bypasses RLS and would prove nothing here):
/// tenant B's audit key must not see tenant A's audit trail, even though both rows live in the
/// same table.
#[tokio::test]
#[cfg(feature = "enterprise")]
async fn an_audit_key_cannot_read_another_tenants_audit_log() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        crate::requests::require_licence(&ctx);
        let tenant_a = setup(&request, &ctx).await;
        let tenant_b = setup(&request, &ctx).await;

        request
            .post("/api/migration-jobs/reindex")
            .add_header(
                "Authorization",
                format!("Bearer {}", tenant_a.migration_key),
            )
            .await
            .assert_status_ok();

        let owner_log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", tenant_a.audit_key))
            .await;
        let owner_entries: Vec<serde_json::Value> = owner_log.json();
        assert_eq!(
            owner_entries.len(),
            1,
            "tenant a must see its own entries: {owner_entries:?}"
        );

        let other_log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", tenant_b.audit_key))
            .await;
        let other_entries: Vec<serde_json::Value> = other_log.json();
        assert!(
            other_entries.is_empty(),
            "tenant b must not see tenant a's audit trail: {other_entries:?}"
        );
    })
    .await;
}
