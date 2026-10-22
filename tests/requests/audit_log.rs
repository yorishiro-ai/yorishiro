use super::{boot_request, boot_request_with_timezone};
use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
use sea_orm::EntityTrait;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;

use super::fixtures::{TenantArgs, TestTenant, issue_api_key};

struct Setup {
    tenant_id: Uuid,
    workspace_id: Uuid,
    /// `Migration` scope, `audit: false`: what triggers the two audited operations, but must not itself be able to read them.
    migration_key: String,
    /// `Read` scope, `audit: true`: the independent grant this suite exists to prove, paired with the lowest scope on purpose to show the grant works with any scope, not only a high one.
    audit_key: String,
}

async fn setup(request: &loco_rs::TestServer, ctx: &loco_rs::app::AppContext) -> Setup {
    let TestTenant {
        tenant_id,
        workspace_id,
        owner_id,
        api_key: migration_key,
    } = TestTenant::create(request, ctx, TenantArgs::default()).await;
    let audit_key = issue_api_key(ctx, workspace_id, owner_id, ApiKeyScope::Read, true).await;

    Setup {
        tenant_id,
        workspace_id,
        migration_key,
        audit_key,
    }
}

/// `set_maintenance` is recorded, and an `audit`-permission key (holding no `Migration` scope at
/// all) can read the row back; the acting key's own scope never had to be raised for this.
#[tokio::test]
async fn set_maintenance_is_recorded_and_readable_by_an_audit_key() {
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

        let set = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .json(&serde_json::json!({ "mode": "read-only", "reason": "audit-log test" }))
            .await;
        assert_eq!(
            set.status_code(),
            StatusCode::OK,
            "response: {:?}",
            set.text()
        );

        // Restore off so a leaked mode doesn't affect an unrelated test on the same throwaway
        // database.
        request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .json(&serde_json::json!({ "mode": "off" }))
            .await;

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
        // Both the read-only switch and the restore-to-off are set_maintenance calls; the most
        // recent (restore) is first.
        assert_eq!(body.len(), 2, "body: {body:?}");
        assert_eq!(body[0]["action"], "set_maintenance");
        assert_eq!(body[1]["action"], "set_maintenance");
        assert_eq!(body[1]["detail"]["mode"], "read_only");
        assert_eq!(body[1]["detail"]["reason"], "audit-log test");
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
    super::fill_defaults::boot_request::<App, _, _>(|request, ctx| async move {
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
            },
        )
        .await
        .expect("overwrite entity");

        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await;
        assert_eq!(
            undo.status_code(),
            StatusCode::ACCEPTED,
            "response: {:?}",
            undo.text()
        );
        let queued_id: Uuid = undo.json::<serde_json::Value>()["job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        super::fill_defaults::finish_via_api(
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
        let dispatch_id: Uuid = response.json::<serde_json::Value>()["job_id"]
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
        let committed_audit = yorishiro::models::api_key_audit_log::list_for_workspace(
            &ctx.db,
            setup.workspace_id,
            Default::default(),
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

/// The independent-grant design this whole feature exists for: a `Migration`-scoped key, the
/// highest scope on the ordered ladder, still cannot read the audit log without the separate
/// `audit` grant.
/// If `audit` had been added as a fifth rung above `Migration` instead, this would 200.
#[tokio::test]
async fn a_migration_scoped_key_without_the_audit_grant_is_refused() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&request, &ctx).await;

        let response = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await;
        assert_eq!(
            response.status_code(),
            403,
            "a Migration-scoped key with no audit grant must be refused: {:?}",
            response.text()
        );
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
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant_a = setup(&request, &ctx).await;
        let tenant_b = setup(&request, &ctx).await;

        let set = request
            .put("/api/system/maintenance")
            .add_header(
                "Authorization",
                format!("Bearer {}", tenant_a.migration_key),
            )
            .json(&serde_json::json!({ "mode": "read-only", "reason": "tenant a only" }))
            .await;
        assert_eq!(
            set.status_code(),
            StatusCode::OK,
            "response: {:?}",
            set.text()
        );
        request
            .put("/api/system/maintenance")
            .add_header(
                "Authorization",
                format!("Bearer {}", tenant_a.migration_key),
            )
            .json(&serde_json::json!({ "mode": "off" }))
            .await;

        let tenant_a_log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", tenant_a.audit_key))
            .await;
        let tenant_a_body: Vec<serde_json::Value> = tenant_a_log.json();
        assert_eq!(
            tenant_a_body.len(),
            2,
            "tenant a must see its own entries: {tenant_a_body:?}"
        );

        let tenant_b_log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", tenant_b.audit_key))
            .await;
        let tenant_b_body: Vec<serde_json::Value> = tenant_b_log.json();
        assert!(
            tenant_b_body.is_empty(),
            "tenant b must not see tenant a's audit trail: {tenant_b_body:?}"
        );
    })
    .await;
}
