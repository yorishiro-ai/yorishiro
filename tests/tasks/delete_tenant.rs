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

async fn assert_disabled_creation_rejected(ctx: &loco_rs::app::AppContext, tenant_id: uuid::Uuid) {
    let before = tenant_tenants::retirement::inventory(&ctx.db, tenant_id)
        .await
        .unwrap();
    for (task, extra) in [
        (
            "create_workspace",
            vec![("name".into(), "must-not-exist".into())],
        ),
        (
            "create_invite",
            vec![
                ("email".into(), "blocked@example.com".into()),
                ("role".into(), "member".into()),
            ],
        ),
    ] {
        let mut args = vec![("tenant_id".into(), tenant_id.to_string())];
        args.extend(extra);
        assert!(
            loco_rs::boot::run_task::<App>(ctx, Some(&task.to_owned()), &Vars::from_cli_args(args))
                .await
                .is_err()
        );
    }
    use sea_orm::{ColumnTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::workspace_invites;
    assert_eq!(
        workspace_invites::Entity::find()
            .filter(yorishiro::models::_entities::workspace_invites::Column::TenantId.eq(tenant_id))
            .count(&ctx.db)
            .await
            .unwrap(),
        0
    );
    let after = tenant_tenants::retirement::inventory(&ctx.db, tenant_id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

#[tokio::test]
#[cfg(feature = "enterprise")]
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
        assert_disabled_creation_rejected(&ctx, target.tenant_id).await;
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

#[tokio::test]
async fn sqlite_retirement_retains_quota_until_deletion_and_preserves_identities() {
    if !crate::require_sqlite_backend() {
        return;
    }
    use sea_orm::TransactionTrait;
    boot_request::<App, _, _>(|request, ctx| async move {
        let target = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let args = vec![
            ("tenant_id".into(), target.tenant_id.to_string()),
            ("confirmed_tenant_id".into(), target.tenant_id.to_string()),
        ];
        assert!(run(&ctx, args.clone()).await.is_err());
        let vars = Vars::from_cli_args(args.clone());
        loco_rs::boot::run_task::<App>(&ctx, Some(&"disable_tenant".to_owned()), &vars)
            .await
            .unwrap();
        let inventory = tenant_tenants::retirement::inventory(&ctx.db, target.tenant_id)
            .await
            .unwrap();
        assert!(inventory.disabled_at.is_some());
        assert_eq!(inventory.api_keys, 0);
        assert!(inventory.workspace_ids.contains(&target.workspace_id));
        assert_disabled_creation_rejected(&ctx, target.tenant_id).await;
        let txn = ctx.db.begin().await.unwrap();
        assert!(matches!(
            tenant_tenants::create_tenant(&txn, "blocked while retained", Some(1000)).await,
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));
        txn.rollback().await.unwrap();
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
        let txn = ctx.db.begin().await.unwrap();
        let replacement =
            tenant_tenants::create_tenant(&txn, "replacement after deletion", Some(1000))
                .await
                .unwrap();
        txn.commit().await.unwrap();
        assert_ne!(replacement.id, target.tenant_id);
    })
    .await;
}
