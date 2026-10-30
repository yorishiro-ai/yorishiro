use loco_rs::task::Vars;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use yorishiro::App;
use yorishiro::models::{api_keys, user_users};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

async fn delete(
    ctx: &loco_rs::app::AppContext,
    id: uuid::Uuid,
    confirmation: uuid::Uuid,
) -> loco_rs::Result<()> {
    loco_rs::boot::run_task::<App>(
        ctx,
        Some(&"delete_user".to_owned()),
        &Vars::from_cli_args(vec![
            ("user_id".into(), id.to_string()),
            ("confirmed_user_id".into(), confirmation.to_string()),
        ]),
    )
    .await
}

#[tokio::test]
async fn operator_user_deletion_requires_confirmation_and_no_memberships() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert!(
            delete(&ctx, tenant.owner_id, tenant.owner_id)
                .await
                .is_err()
        );
        assert!(
            user_users::Entity::find_by_id(tenant.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        let user =
            user_users::create_user(&ctx.db, "detached@example.com", "long-test-password", None)
                .await
                .unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        // Simulate retained attributed credentials after out-of-band membership cleanup.
        let mut attributed: api_keys::ActiveModel = api_keys::Entity::find_by_id(key.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        attributed.user_id = ActiveValue::Set(Some(user.id));
        attributed.update(&ctx.db).await.unwrap();
        assert!(delete(&ctx, user.id, uuid::Uuid::new_v4()).await.is_err());
        delete(&ctx, user.id, user.id).await.unwrap();
        assert!(
            user_users::Entity::find_by_id(user.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        let stored = api_keys::Entity::find_by_id(key.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.revoked_at.is_some());
        assert!(stored.user_id.is_none());
        assert!(
            user_users::Entity::find_by_id(tenant.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        assert!(delete(&ctx, user.id, user.id).await.is_err());
        loco_rs::boot::run_task::<App>(
            &ctx,
            Some(&"list_users".to_owned()),
            &Vars::from_cli_args(vec![]),
        )
        .await
        .unwrap();
    })
    .await;
}
