use axum::http::StatusCode;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, TransactionTrait};
use yorishiro::{
    App,
    models::{api_keys, tenancy, tenant_memberships},
};

use crate::requests::{
    boot_request,
    fixtures::{TenantArgs, TestTenant, unique_email},
};

#[tokio::test]
async fn tenant_key_issuance_rechecks_admin_after_waiting_for_demotion() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.unwrap();
        let admin = tenancy::register_member(&txn, tenant.tenant_id, &unique_email("issuing-admin"),
            "hunter2-hunter2", None, tenant_memberships::MembershipRole::Admin).await.unwrap();
        txn.commit().await.unwrap();
        let issuer = api_keys::Entity::create_tenant_api_key(&ctx.db, tenant.tenant_id,
            api_keys::ApiKeyScope::Migration, Some(admin.id), false, "demotion-issuer").await.unwrap();
        let txn = ctx.db.begin().await.unwrap();
        let fence = format!("tenant-membership:{}", tenant.tenant_id);
        yorishiro::db::lock_for_update(&txn, &fence).await.unwrap();
        let issue = async {
            request.post("/api/api-keys")
                .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
                .add_header(api_keys::WORKSPACE_HEADER, tenant.workspace_id.to_string())
                .json(&serde_json::json!({"name":"must-not-exist", "scope":"read", "tenant_scoped":true})).await
        };
        let demote = async {
            // Observe the real fence wait rather than guessing how long the HTTP request needs.
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    let row = ctx.db.query_one_raw(sea_orm::Statement::from_sql_and_values(
                        sea_orm::DatabaseBackend::Postgres,
                        "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295)) AS waiting",
                        [fence.clone().into()],
                    )).await.unwrap().unwrap();
                    if row.try_get::<bool>("", "waiting").unwrap() { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await.expect("issuance must wait on the membership fence");
            use yorishiro::models::_entities::tenant_memberships::Column;
            tenant_memberships::Entity::update_many()
                .col_expr(Column::Role, sea_orm::sea_query::Expr::value("member"))
                .filter(Column::TenantId.eq(tenant.tenant_id)).filter(Column::UserId.eq(admin.id))
                .exec(&txn).await.unwrap();
            txn.commit().await.unwrap();
        };
        let (response, ()) = tokio::join!(issue, demote);
        response.assert_status(StatusCode::FORBIDDEN);
        assert!(api_keys::Entity::find()
            .filter(yorishiro::models::_entities::api_keys::Column::UserId.eq(admin.id))
            .filter(yorishiro::models::_entities::api_keys::Column::Name.eq("must-not-exist"))
            .one(&ctx.db).await.unwrap().is_none());
    }).await;
}
