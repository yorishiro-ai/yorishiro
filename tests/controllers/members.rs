use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
#[tokio::test]
#[cfg(feature = "enterprise")]
async fn competing_tenant_memberships_allow_only_one() {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::tenant_memberships;

    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let first = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let second = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("competing-tenants"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        let barrier = tokio::sync::Barrier::new(2);
        let a = async {
            barrier.wait().await;
            tenant_memberships::add_member(&ctx.db, first.tenant_id, user.id).await
        };
        let b = async {
            barrier.wait().await;
            tenant_memberships::add_member(&ctx.db, second.tenant_id, user.id).await
        };
        let (a, b) = tokio::join!(a, b);
        assert_ne!(a.is_ok(), b.is_ok(), "{a:?}, {b:?}");
        let other_tenant = if a.is_ok() {
            second.tenant_id
        } else {
            first.tenant_id
        };
        assert!(matches!(
            if a.is_err() { a } else { b },
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));
        assert_eq!(
            tenant_memberships::Entity::find()
                .filter(Column::UserId.eq(user.id))
                .count(&ctx.db)
                .await
                .unwrap(),
            1
        );
        // Direct write (b): the test proves the database unique constraint, which `add_member` would reject first.
        let duplicate = tenant_memberships::Entity::insert(tenant_memberships::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(other_tenant),
            user_id: sea_orm::ActiveValue::Set(user.id),
            ..Default::default()
        })
        .exec(&ctx.db)
        .await
        .unwrap_err();
        assert!(matches!(
            duplicate.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ));
    })
    .await;
}

#[tokio::test]
async fn root_removal_conflicts_and_preserves_the_root() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::tenant_memberships;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let removed =
            tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, tenant.owner_id).await;
        assert!(
            matches!(removed, Err(yorishiro::YorishiroError::Conflict { .. })),
            "root removal must conflict: {removed:?}"
        );
        let _membership = tenant_memberships::Entity::find()
            .filter(Column::TenantId.eq(tenant.tenant_id))
            .filter(Column::UserId.eq(tenant.owner_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .expect("root membership survives");
        let root =
            yorishiro::models::_entities::iam_tenant_roots::Entity::find_by_id(tenant.tenant_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("root row survives");
        assert_eq!(root.user_id, tenant.owner_id);
    })
    .await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
#[expect(
    clippy::too_many_lines,
    reason = "the gated issuance-removal race shares setup and verifies both durable revocation and HTTP rejection after membership is restored"
)]
async fn issuance_and_removal_are_serialized(tenant_key: bool) {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::api_keys::Column;
    use yorishiro::models::{api_keys, tenant_memberships};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("issuance-race"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id)
            .await
            .unwrap();
        crate::requests::fixtures::grant_workspace(
            &ctx,
            tenant.workspace_id,
            user.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Viewer,
        )
        .await;
        if tenant_key {
            let created = api_keys::Entity::create_tenant_api_key(
                &ctx.db,
                tenant.tenant_id,
                ApiKeyScope::Read,
                Some(user.id),
                false,
                "membership-race-probe",
            )
            .await
            .expect("tenant issuance must succeed before removal, including SQLite ID generation");
            let stored = api_keys::Entity::find_by_id(created.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("issued tenant key must be persisted");
            assert!(!stored.id.is_nil());
            assert_eq!(stored.user_id, Some(user.id));
            assert_eq!(stored.workspace_id, None);
            api_keys::Entity::revoke(&ctx.db, created.id).await.unwrap();
        }
        let broad = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Write,
            Some(user.id),
            false,
        )
        .await
        .unwrap();
        api_keys::Entity::revoke(&ctx.db, broad.id).await.unwrap();
        let barrier = tokio::sync::Barrier::new(2);
        let issue = async {
            barrier.wait().await;
            if tenant_key {
                return api_keys::Entity::create_tenant_api_key(
                    &ctx.db,
                    tenant.tenant_id,
                    ApiKeyScope::Read,
                    Some(user.id),
                    false,
                    "membership-race",
                )
                .await
                .map(|key| key.plaintext);
            }
            api_keys::Entity::create_api_key(
                &ctx.db,
                tenant.workspace_id,
                ApiKeyScope::Read,
                Some(user.id),
                false,
            )
            .await
            .map(|key| key.plaintext)
        };
        let remove = async {
            barrier.wait().await;
            tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, user.id).await
        };
        let (issued, removed) = tokio::join!(issue, remove);
        removed.unwrap();
        let keys = api_keys::Entity::find()
            .filter(Column::TenantId.eq(tenant.tenant_id))
            .filter(Column::UserId.eq(user.id))
            .filter(Column::RevokedAt.is_null())
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            keys, 0,
            "removal must revoke a racing issuance or prevent its insertion"
        );
        assert!(
            api_keys::Entity::create_api_key(
                &ctx.db,
                tenant.workspace_id,
                ApiKeyScope::Read,
                Some(user.id),
                false
            )
            .await
            .is_err()
        );
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id)
            .await
            .unwrap();
        if let Ok(key) = issued {
            let response = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
        } else {
            assert!(
                matches!(issued, Err(yorishiro::YorishiroError::Unauthenticated)),
                "unexpected issuance result: {issued:?}"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn workspace_key_issuance_cannot_survive_concurrent_membership_removal() {
    issuance_and_removal_are_serialized(false).await;
}

#[tokio::test]
async fn tenant_key_issuance_cannot_survive_concurrent_membership_removal() {
    issuance_and_removal_are_serialized(true).await;
}

#[tokio::test]
async fn tenant_users_sign_in_with_tenant_id_and_name_until_removed() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {tenant_admin_key}");
        request
            .put(&format!("/api/members/users/{}/password", tenant.owner_id))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"password":"must-not-replace-root-password"}))
            .await
            .assert_status(StatusCode::CONFLICT);
        let create = |body: serde_json::Value| {
            request
                .post("/api/members/users")
                .add_header("Authorization", &auth)
                .json(&body)
        };
        let user_name = format!("alice-{}", uuid::Uuid::new_v4());
        let password = "tenant-user-password";
        let created = create(serde_json::json!({"user_name":user_name,"password":password})).await;
        created.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = created.json();
        assert!(body["email"].is_null());
        assert_eq!(body["user_name"], user_name);
        let user_id = body["user_id"].as_str().unwrap().to_owned();

        create(serde_json::json!({"user_name":user_name,"password":password}))
            .await
            .assert_status(StatusCode::CONFLICT);
        create(serde_json::json!({"user_name":"carol","password":"short"}))
            .await
            .assert_status(StatusCode::CONFLICT);

        let login = |body: serde_json::Value| {
            request
                .post("/auth/tenant/login")
                .json(&{ let mut body = body; body["tenant_id"] = serde_json::json!(tenant.tenant_id); body })
        };
        request
            .post("/auth/tenant/login")
            .json(&serde_json::json!({"user_name":user_name,"password":password}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .post("/auth/tenant/login")
            .json(&serde_json::json!({"tenant_id":uuid::Uuid::new_v4(),"user_name":user_name,"password":password}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let signed_in = login(serde_json::json!({"user_name":user_name,"password":password})).await;
        signed_in.assert_status(StatusCode::OK);
        assert_eq!(
            signed_in.json::<serde_json::Value>()["tenant_id"],
            tenant.tenant_id.to_string()
        );
        login(serde_json::json!({"user_name":user_name,"password":"wrong-password-here"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        login(
            serde_json::json!({"email":"a@example.com","user_name":user_name,"password":password}),
        )
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);

        let old_key = signed_in.json::<serde_json::Value>()["api_key"]
            .as_str()
            .unwrap()
            .to_owned();
        request
            .put(&format!("/api/members/users/{user_id}/password"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"password":"replacement-password"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/members")
            .add_header("Authorization", &format!("Bearer {old_key}"))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        login(serde_json::json!({"user_name":user_name,"password":password}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        login(serde_json::json!({"user_name":user_name,"password":"replacement-password"}))
            .await
            .assert_status(StatusCode::OK);

        // Removing the membership removes the account.
        request
            .delete(&format!("/api/members/{user_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        login(serde_json::json!({"user_name":user_name,"password":"replacement-password"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}
