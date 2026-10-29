#![cfg(feature = "enterprise")]

use loco_rs::task::Vars;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::_entities::tenant_retirement_events::Column;
use yorishiro::models::{tenant_retirement_events, tenant_tenants};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

#[tokio::test]
async fn operator_disablement_requires_confirmation_and_is_retry_safe() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let task = Some("disable_tenant".to_owned());
        for args in [
            vec![("tenant_id".into(), target.tenant_id.to_string())],
            vec![
                ("tenant_id".into(), target.tenant_id.to_string()),
                ("confirmed_tenant_id".into(), other.tenant_id.to_string()),
            ],
        ] {
            assert!(
                loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &Vars::from_cli_args(args))
                    .await
                    .is_err()
            );
        }
        assert!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at
                .is_none()
        );
        let vars = Vars::from_cli_args(vec![
            ("tenant_id".into(), target.tenant_id.to_string()),
            ("confirmed_tenant_id".into(), target.tenant_id.to_string()),
        ]);
        loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &vars)
            .await
            .unwrap();
        let disabled_at = tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
            .await
            .unwrap()
            .disabled_at;
        assert!(disabled_at.is_some());
        loco_rs::boot::run_task::<App>(&ctx, task.as_ref(), &vars)
            .await
            .unwrap();
        assert_eq!(
            tenant_tenants::Model::find_by_id(&ctx.db, target.tenant_id)
                .await
                .unwrap()
                .disabled_at,
            disabled_at
        );
        let events = tenant_retirement_events::Entity::find()
            .filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, "disabled");
        assert_eq!(events[0].actor_user_id, None);
        let inventory = tenant_tenants::retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert_eq!(inventory.api_keys, 0);
        assert!(inventory.workspace_ids.contains(&target.workspace_id));
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", target.api_key))
            .await
            .assert_status(axum::http::StatusCode::UNAUTHORIZED);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}
