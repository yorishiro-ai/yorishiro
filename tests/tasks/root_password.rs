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
    })
    .await;
}

#[tokio::test]
async fn postgres_root_reset_fences_login_waiting_with_verified_old_password() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        use sea_orm::{QuerySelect, TransactionTrait};

        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let row_lock = ctx.db.begin().await.unwrap();
        let before = user_users::Entity::find_by_id(tenant.owner_id)
            .lock_exclusive()
            .one(&row_lock)
            .await
            .unwrap()
            .unwrap();
        let fence = format!("tenant-membership:{}", tenant.tenant_id);
        let reset = user_users::reset_root_password_by_operator(
            &ctx.db,
            tenant.owner_id,
            tenant.owner_id,
            NEW_PASSWORD,
        );
        let login_after_reset_locks = async {
            // Reset owns the membership fence but cannot update the password until we release the row.
            wait_for_membership_lock(&ctx.db, &fence, true).await;
            let login = yorishiro::models::tenancy::issue_login_key(
                &ctx.db,
                tenant.owner_id,
                before.password_hash.as_deref(),
                None,
            );
            let release_reset = async {
                wait_for_membership_lock(&ctx.db, &fence, false).await;
                row_lock.rollback().await.unwrap();
            };
            let (login, ()) = tokio::join!(login, release_reset);
            assert!(matches!(
                login,
                Err(yorishiro::error::YorishiroError::Unauthenticated)
            ));
        };
        let (reset, ()) = tokio::join!(reset, login_after_reset_locks);
        reset.unwrap();
        let after = user_users::Entity::find_by_id(tenant.owner_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert!(loco_rs::hash::verify_password(
            NEW_PASSWORD,
            after.password_hash.as_deref().unwrap()
        ));
    })
    .await;
}

async fn wait_for_membership_lock(db: &sea_orm::DatabaseConnection, fence: &str, granted: bool) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted = $2 AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS observed",
                    [fence.into(), granted.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            if row.try_get::<bool>("", "observed").unwrap() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected membership fence holder or waiter was not observed");
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
