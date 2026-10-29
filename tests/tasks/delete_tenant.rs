#![cfg(feature = "enterprise")]

use loco_rs::task::Vars;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::{tenant_retirement_events, tenant_tenants, user_users};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

async fn run(ctx: &loco_rs::app::AppContext, args: Vec<(String, String)>) -> loco_rs::Result<()> {
    loco_rs::boot::run_task::<App>(
        ctx,
        Some(&"delete_tenant".to_owned()),
        &Vars::from_cli_args(args),
    )
    .await
}

#[tokio::test]
async fn delete_task_requires_confirmation_and_disablement_and_supports_retry() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert!(run(&ctx, vec![]).await.is_err());
        assert!(
            run(&ctx, vec![("tenant_id".into(), "invalid".into())])
                .await
                .is_err()
        );
        let args = vec![
            ("tenant_id".into(), target.tenant_id.to_string()),
            ("confirmed_tenant_id".into(), target.tenant_id.to_string()),
        ];
        assert!(run(&ctx, vec![args[0].clone()]).await.is_err());
        assert!(
            run(
                &ctx,
                vec![
                    args[0].clone(),
                    ("confirmed_tenant_id".into(), other.tenant_id.to_string())
                ]
            )
            .await
            .is_err()
        );
        assert!(run(&ctx, args.clone()).await.is_err());
        assert!(
            tenant_tenants::Entity::find_by_id(target.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        tenant_tenants::retirement::disable_by_operator(
            &ctx.db,
            target.tenant_id,
            target.tenant_id,
        )
        .await
        .unwrap();
        run(&ctx, args.clone()).await.unwrap();
        run(&ctx, args).await.unwrap();
        assert!(
            tenant_tenants::Entity::find_by_id(target.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            user_users::Entity::find_by_id(target.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        use yorishiro::models::_entities::tenant_retirement_events::Column;
        let events = tenant_retirement_events::Entity::find()
            .filter(Column::TenantId.eq(target.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events
                .iter()
                .filter(|event| event.action == "deleted")
                .count(),
            1
        );
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", other.api_key))
            .await
            .assert_status(axum::http::StatusCode::OK);
    })
    .await;
}
