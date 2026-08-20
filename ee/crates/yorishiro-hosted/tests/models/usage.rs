use crate::models::usage::compute_tenant_usage;
use serde_json::json;
use sqlx::PgPool;
use yorishiro_core::db::TenantDb;
use yorishiro_core::metaschema::MetaSchemaDefinition;
use yorishiro_core::models::entities::{self, CreateEntityInput};
use yorishiro_core::models::schemas;
use yorishiro_core::models::tenancy;

fn note_schema() -> MetaSchemaDefinition {
    serde_json::from_value(json!({
        "name": "notes",
        "entity_types": {
            "note": {
                "fields": { "title": { "type": "string", "required": true } }
            }
        }
    }))
    .unwrap()
}

#[sqlx::test(migrations = "../../../migrations")]
async fn counts_workspaces_members_and_entities_across_a_tenant(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace_a = tenancy::create_workspace(&pool, tenant.id, "prod", None, None, None)
        .await
        .unwrap();
    tenancy::create_workspace(&pool, tenant.id, "staging", None, None, None)
        .await
        .unwrap();

    let mut identity_conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut *identity_conn, "owner@acme.test", "password123", None)
        .await
        .unwrap();
    tenancy::add_member(
        &mut *identity_conn,
        tenant.id,
        user.id,
        tenancy::MembershipRole::Owner,
    )
    .await
    .unwrap();

    let db = TenantDb::new(pool.clone());
    let mut conn = db
        .acquire_for_workspace(tenant.id, workspace_a.id)
        .await
        .unwrap();
    schemas::create_schema(&mut conn, tenant.id, workspace_a.id, note_schema())
        .await
        .unwrap();
    for title in ["first", "second"] {
        entities::create(
            &mut conn,
            workspace_a.id,
            CreateEntityInput {
                schema_name: "notes".into(),
                entity_type: "note".into(),
                data: json!({ "title": title }),
            },
            None,
        )
        .await
        .unwrap();
    }

    let usage = compute_tenant_usage(&pool, tenant.id).await.unwrap();
    assert_eq!(usage.tenant_id, tenant.id);
    assert_eq!(usage.workspace_count, 2);
    assert_eq!(usage.member_count, 1);
    assert_eq!(usage.entity_count, 2);
}

#[sqlx::test(migrations = "../../../migrations")]
async fn a_fresh_tenant_has_zero_usage(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "empty-co", None)
        .await
        .unwrap();

    let usage = compute_tenant_usage(&pool, tenant.id).await.unwrap();
    assert_eq!(usage.workspace_count, 0);
    assert_eq!(usage.member_count, 0);
    assert_eq!(usage.entity_count, 0);
}
