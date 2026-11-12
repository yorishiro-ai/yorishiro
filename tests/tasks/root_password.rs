use loco_rs::task::Vars;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use yorishiro::App;
use yorishiro::models::{api_keys, tenant_memberships, tenant_tenants, user_users};

use crate::requests::fixtures::{TenantArgs, TestTenant};
use crate::requests::{boot_request, boot_request_isolated};

const NEW_PASSWORD: &str = "new-root-password-123";

#[tokio::test]
async fn root_reset_revokes_attributed_keys_and_fences_verified_old_password() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let before = user_users::Entity::find_by_id(tenant.owner_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let old_hash = before.password_hash.clone().unwrap();
        let login = yorishiro::models::tenancy::issue_login_key(
            &ctx.db,
            tenant.owner_id,
            Some(&old_hash),
            None,
        )
        .await
        .unwrap()
        .0;
        let attributed = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Read,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap();
        let service = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        user_users::reset_root_password_by_operator(
            &ctx.db,
            tenant.owner_id,
            tenant.owner_id,
            NEW_PASSWORD,
        )
        .await
        .unwrap();
        let after = user_users::Entity::find_by_id(tenant.owner_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(loco_rs::hash::verify_password(
            NEW_PASSWORD,
            after.password_hash.as_deref().unwrap()
        ));
        assert_eq!(after.email, before.email);
        assert_eq!(after.display_name, before.display_name);
        assert_eq!(after.tenant_id, before.tenant_id);
        assert_eq!(after.user_name, before.user_name);
        let membership = tenant_memberships::Entity::find()
            .filter(
                yorishiro::models::_entities::tenant_memberships::Column::UserId
                    .eq(tenant.owner_id),
            )
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(membership.role, "owner");
        for id in [login.id, attributed.id] {
            assert!(
                api_keys::Entity::find_by_id(id)
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap()
                    .revoked_at
                    .is_some()
            );
        }
        assert!(
            api_keys::Entity::find_by_id(service.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .revoked_at
                .is_none()
        );
        assert!(
            yorishiro::models::tenancy::issue_login_key(
                &ctx.db,
                tenant.owner_id,
                Some(&old_hash),
                None
            )
            .await
            .is_err()
        );
        assert!(
            yorishiro::models::tenancy::issue_login_key(
                &ctx.db,
                tenant.owner_id,
                after.password_hash.as_deref(),
                None
            )
            .await
            .is_ok()
        );
        let barrier = tokio::sync::Barrier::new(2);
        let (reset, login) = tokio::join!(
            async {
                barrier.wait().await;
                user_users::reset_root_password_by_operator(
                    &ctx.db,
                    tenant.owner_id,
                    tenant.owner_id,
                    "another-root-password",
                )
                .await
            },
            async {
                barrier.wait().await;
                yorishiro::models::tenancy::issue_login_key(
                    &ctx.db,
                    tenant.owner_id,
                    after.password_hash.as_deref(),
                    None,
                )
                .await
            },
        );
        reset.unwrap();
        if let Ok((key, _, _)) = login {
            assert!(
                api_keys::Entity::find_by_id(key.id)
                    .one(&ctx.db)
                    .await
                    .unwrap()
                    .unwrap()
                    .revoked_at
                    .is_some()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn root_reset_rejects_unsafe_targets_without_changing_credentials() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let original = user_users::Entity::find_by_id(tenant.owner_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Read,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap();
        let absent = uuid::Uuid::new_v4();
        assert!(
            user_users::reset_root_password_by_operator(&ctx.db, absent, absent, NEW_PASSWORD)
                .await
                .is_err()
        );
        for (confirmation, password) in [
            (uuid::Uuid::new_v4(), NEW_PASSWORD),
            (tenant.owner_id, "short"),
        ] {
            assert!(
                user_users::reset_root_password_by_operator(
                    &ctx.db,
                    tenant.owner_id,
                    confirmation,
                    password
                )
                .await
                .is_err()
            );
        }
        let detached = user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("detached-reset"),
            "original-password",
            None,
        )
        .await
        .unwrap();
        assert!(
            user_users::reset_root_password_by_operator(
                &ctx.db,
                detached.id,
                detached.id,
                NEW_PASSWORD
            )
            .await
            .is_err()
        );
        let mut passwordless: user_users::ActiveModel = original.clone().into();
        passwordless.password_hash = ActiveValue::Set(None);
        passwordless.oauth_provider = ActiveValue::Set(Some("github".into()));
        passwordless.oauth_subject_id = ActiveValue::Set(Some(uuid::Uuid::new_v4().to_string()));
        passwordless.update(&ctx.db).await.unwrap();
        assert!(
            user_users::reset_root_password_by_operator(
                &ctx.db,
                tenant.owner_id,
                tenant.owner_id,
                NEW_PASSWORD
            )
            .await
            .is_err()
        );
        let mut local: user_users::ActiveModel = original.clone().into();
        local.tenant_id = ActiveValue::Set(Some(tenant.tenant_id));
        local.user_name = ActiveValue::Set(Some("local-reset-target".into()));
        local.email = ActiveValue::Set(None);
        local.oauth_provider = ActiveValue::Set(None);
        local.oauth_subject_id = ActiveValue::Set(None);
        local.password_hash = ActiveValue::Set(original.password_hash.clone());
        local.update(&ctx.db).await.unwrap();
        assert!(
            user_users::reset_root_password_by_operator(
                &ctx.db,
                tenant.owner_id,
                tenant.owner_id,
                NEW_PASSWORD
            )
            .await
            .is_err()
        );
        let mut restored: user_users::ActiveModel = original.clone().into();
        restored.tenant_id = ActiveValue::Set(None);
        restored.user_name = ActiveValue::Set(None);
        restored.password_hash = ActiveValue::Set(original.password_hash.clone());
        restored.email = ActiveValue::Set(original.email.clone());
        restored.update(&ctx.db).await.unwrap();
        let mut disabled: tenant_tenants::ActiveModel =
            tenant_tenants::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .into();
        disabled.disabled_at = ActiveValue::Set(Some(chrono::Utc::now().fixed_offset()));
        disabled.update(&ctx.db).await.unwrap();
        assert!(
            user_users::reset_root_password_by_operator(
                &ctx.db,
                tenant.owner_id,
                tenant.owner_id,
                NEW_PASSWORD
            )
            .await
            .is_err()
        );
        assert_eq!(
            user_users::Entity::find_by_id(tenant.owner_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .password_hash,
            original.password_hash
        );
        assert!(
            api_keys::Entity::find_by_id(key.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .revoked_at
                .is_none()
        );
    })
    .await;
}

#[tokio::test]
async fn root_reset_task_rejects_argv_secrets_and_bad_confirmation_before_stdin() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        let id = uuid::Uuid::new_v4();
        for args in [
            vec![
                ("user_id".into(), id.to_string()),
                ("confirmed_user_id".into(), id.to_string()),
                ("password".into(), NEW_PASSWORD.into()),
            ],
            vec![
                ("user_id".into(), "invalid".into()),
                ("confirmed_user_id".into(), id.to_string()),
            ],
            vec![
                ("user_id".into(), id.to_string()),
                ("confirmed_user_id".into(), uuid::Uuid::new_v4().to_string()),
            ],
        ] {
            assert!(
                loco_rs::boot::run_task::<App>(
                    &ctx,
                    Some(&"reset_root_password".into()),
                    &Vars::from_cli_args(args)
                )
                .await
                .is_err()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn root_reset_rolls_back_password_when_key_revocation_fails() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        use sea_orm::ConnectionTrait;
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let before = user_users::Entity::find_by_id(tenant.owner_id).one(&ctx.db).await.unwrap().unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db, tenant.workspace_id, api_keys::ApiKeyScope::Read, Some(tenant.owner_id), false,
        ).await.unwrap();
        ctx.db.execute_unprepared(
            "CREATE TRIGGER fail_reset_revocation BEFORE UPDATE OF revoked_at ON api_keys BEGIN SELECT RAISE(ABORT, 'injected revocation failure'); END",
        ).await.unwrap();
        assert!(user_users::reset_root_password_by_operator(&ctx.db, tenant.owner_id, tenant.owner_id, NEW_PASSWORD).await.is_err());
        assert_eq!(user_users::Entity::find_by_id(tenant.owner_id).one(&ctx.db).await.unwrap().unwrap().password_hash, before.password_hash);
        assert!(api_keys::Entity::find_by_id(key.id).one(&ctx.db).await.unwrap().unwrap().revoked_at.is_none());
    }).await;
}
