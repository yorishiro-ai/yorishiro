use super::boot_request;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    tenant_id: Uuid,
    workspace_id: Uuid,
    /// `Migration` scope, `audit: false`: what triggers the two audited operations, but must not itself be able to read them.
    migration_key: String,
    /// `Read` scope, `audit: true`: the independent grant this suite exists to prove, paired with the lowest scope on purpose to show the grant works with any scope, not only a high one.
    audit_key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext, tenant_name: &str) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(tenant_name.into()),
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
        &format!("owner-{tenant_name}@example.com"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");

    let migration_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Migration,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue migration key")
    .plaintext;
    let audit_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Read,
        Some(owner.id),
        true,
    )
    .await
    .expect("issue audit key")
    .plaintext;

    Setup {
        tenant_id: tenant.id,
        workspace_id: workspace.id,
        migration_key,
        audit_key,
    }
}

/// `set_maintenance` is recorded, and an `audit`-permission key (holding no `Migration` scope at
/// all) can read the row back; the acting key's own scope never had to be raised for this.
#[tokio::test]
#[serial]
async fn set_maintenance_is_recorded_and_readable_by_an_audit_key() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx, "acme").await;

        let set = request
            .put("/api/system/maintenance")
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .json(&serde_json::json!({ "mode": "read-only", "reason": "audit-log test" }))
            .await;
        assert_eq!(set.status_code(), 200, "response: {:?}", set.text());

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
        assert_eq!(log.status_code(), 200, "response: {:?}", log.text());
        let body: Vec<serde_json::Value> = log.json();
        // Both the read-only switch and the restore-to-off are set_maintenance calls; the most
        // recent (restore) is first.
        assert_eq!(body.len(), 2, "body: {body:?}");
        assert_eq!(body[0]["action"], "set_maintenance");
        assert_eq!(body[1]["action"], "set_maintenance");
        assert_eq!(body[1]["detail"]["mode"], "read_only");
        assert_eq!(body[1]["detail"]["reason"], "audit-log test");
    })
    .await;
}

/// `undo_migration_job` is recorded on the same transaction as the undo itself.
#[tokio::test]
#[serial]
async fn undo_migration_job_is_recorded() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx, "acme").await;

        let definition = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .expect("parse definition");
        yorishiro::models::content_schemas::create_schema(
            &ctx.db,
            setup.tenant_id,
            setup.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .expect("create schema");
        let entity = yorishiro::models::content_entities::create(
            &ctx.db,
            setup.workspace_id,
            yorishiro::models::content_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let job_id = Uuid::new_v4();
        yorishiro::models::content_entities::snapshot(
            &ctx.db,
            setup.workspace_id,
            entity.id,
            job_id,
        )
        .await
        .expect("snapshot entity");
        yorishiro::models::content_entities::update(
            &ctx.db,
            setup.workspace_id,
            entity.id,
            serde_json::json!({ "title": "overwritten" }),
            None,
        )
        .await
        .expect("overwrite entity");

        let undo = request
            .post(&format!("/api/migration-jobs/{job_id}/undo"))
            .add_header("Authorization", format!("Bearer {}", setup.migration_key))
            .await;
        assert_eq!(undo.status_code(), 200, "response: {:?}", undo.text());

        let log = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {}", setup.audit_key))
            .await;
        let body: Vec<serde_json::Value> = log.json();
        assert_eq!(body.len(), 1, "body: {body:?}");
        assert_eq!(body[0]["action"], "undo_migration_job");
        assert_eq!(body[0]["detail"]["job_id"], job_id.to_string());
        assert_eq!(body[0]["detail"]["restored"], 1);
    })
    .await;
}

/// The independent-grant design this whole feature exists for: a `Migration`-scoped key, the
/// highest scope on the ordered ladder, still cannot read the audit log without the separate
/// `audit` grant.
/// If `audit` had been added as a fifth rung above `Migration` instead, this would 200.
#[tokio::test]
#[serial]
async fn a_migration_scoped_key_without_the_audit_grant_is_refused() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx, "acme").await;

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
#[serial]
async fn an_audit_key_cannot_read_another_tenants_audit_log() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant_a = setup(&ctx, "acme").await;
        let tenant_b = setup(&ctx, "beta").await;

        let set = request
            .put("/api/system/maintenance")
            .add_header(
                "Authorization",
                format!("Bearer {}", tenant_a.migration_key),
            )
            .json(&serde_json::json!({ "mode": "read-only", "reason": "tenant a only" }))
            .await;
        assert_eq!(set.status_code(), 200, "response: {:?}", set.text());
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
