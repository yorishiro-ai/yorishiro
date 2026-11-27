use loco_rs::task::Vars;
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait};
use yorishiro::App;
use yorishiro::models::tenant_tenants::retirement;
use yorishiro::models::{access_keys, user_users};

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
async fn operator_user_deletion_requires_confirmation_and_schedules_root_tenants() {
    use sea_orm::{ColumnTrait, QueryFilter};
    use yorishiro::models::_entities::{iam_tenant_roots, tenant_memberships, tenant_tenants};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert!(
            delete(&ctx, tenant.owner_id, uuid::Uuid::new_v4())
                .await
                .is_err()
        );
        delete(&ctx, tenant.owner_id, tenant.owner_id)
            .await
            .unwrap();
        assert!(
            user_users::Entity::find_by_id(tenant.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none(),
            "the root account is deleted"
        );
        assert!(
            iam_tenant_roots::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none(),
            "the root row is deleted"
        );
        assert!(
            tenant_memberships::Entity::find()
                .filter(tenant_memberships::Column::UserId.eq(tenant.owner_id))
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none(),
            "the membership cascades with the account"
        );
        let scheduled = tenant_tenants::Entity::find_by_id(tenant.tenant_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .expect("the tenant survives its grace period")
            .delete_scheduled_at
            .expect("root deletion schedules the tenant");
        assert!(
            scheduled.to_utc() > chrono::Utc::now(),
            "the deadline lies in the future"
        );

        let user =
            user_users::create_user(&ctx.db, "detached@example.com", "long-test-password", None)
                .await
                .unwrap();
        let key = crate::requests::fixtures::create_key(&ctx.db, tenant.workspace_id, None)
            .await
            .unwrap();
        // Simulate retained attributed credentials after out-of-band membership cleanup.
        let mut attributed: access_keys::ActiveModel = access_keys::Entity::find_by_id(key.id)
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
        let stored = access_keys::Entity::find_by_id(key.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(stored.revoked_at.is_some());
        assert!(stored.user_id.is_none());
        assert!(delete(&ctx, user.id, user.id).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn deleting_the_root_reaps_the_tenant_once_the_deadline_passes() {
    use sea_orm::{ColumnTrait, QueryFilter};
    use yorishiro::models::_entities::{tenant_retirement_events, tenant_tenants};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let created = request
            .post("/api/users")
            .add_header("Authorization", format!("Bearer {tenant_key}"))
            .json(&serde_json::json!({
                "user_name": "reap-delegate",
                "password": "reap-delegate-pass",
            }))
            .await;
        created.assert_status(axum::http::StatusCode::CREATED);
        let body: serde_json::Value = created.json();
        let delegate_id = uuid::Uuid::parse_str(body["user_id"].as_str().unwrap()).unwrap();

        delete(&ctx, tenant.owner_id, tenant.owner_id)
            .await
            .unwrap();
        assert_eq!(
            retirement::reap_due(&ctx.db, chrono::Utc::now())
                .await
                .unwrap(),
            0
        );
        assert!(
            tenant_tenants::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some(),
            "the tenant survives until its deadline"
        );
        retirement::reap_due(&ctx.db, chrono::Utc::now() + chrono::Duration::days(8))
            .await
            .unwrap();
        assert!(
            tenant_tenants::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none(),
            "the tenant is deleted once its deadline passes"
        );
        assert!(
            user_users::Entity::find_by_id(tenant.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            user_users::Entity::find_by_id(delegate_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none(),
            "tenant-bound users are deleted with the tenant"
        );
        let events = tenant_retirement_events::Entity::find()
            .filter(tenant_retirement_events::Column::TenantId.eq(tenant.tenant_id))
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.action == "delete_scheduled"),
            "the scheduling is recorded: {events:?}"
        );
        assert!(
            events.iter().any(|event| event.action == "deleted"),
            "the deletion is recorded: {events:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn deleting_the_root_of_a_disabled_tenant_deletes_the_tenant_at_once() {
    use yorishiro::models::_entities::tenant_tenants;
    use yorishiro::models::tenant_tenants::retirement;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        retirement::disable_by_operator(&ctx.db, tenant.tenant_id, tenant.tenant_id)
            .await
            .unwrap();
        delete(&ctx, tenant.owner_id, tenant.owner_id)
            .await
            .unwrap();
        assert!(
            tenant_tenants::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_none()
        );
    })
    .await;
}
