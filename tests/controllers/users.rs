use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use yorishiro::App;
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
        crate::requests::require_licence(&ctx);
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
            id: sea_orm::ActiveValue::Set(uuid::Uuid::now_v7()),
            tenant_id: sea_orm::ActiveValue::Set(other_tenant),
            user_id: sea_orm::ActiveValue::Set(user.id),
            created_at: sea_orm::ActiveValue::Set(chrono::Utc::now().into()),
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
    reason = "one membership history compares racing issuance with removal for both key kinds"
)]
async fn issuance_and_removal_are_serialized(tenant_key: bool) {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::access_keys::Column;
    use yorishiro::models::{access_keys, tenant_memberships};

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
        if tenant_key {
            let created = access_keys::Entity::create_tenant_access_key(
                &ctx.db,
                tenant.tenant_id,
                Some(user.id),
                "membership-race-probe",
            )
            .await
            .expect("tenant issuance must succeed before removal, including SQLite ID generation");
            let stored = access_keys::Entity::find_by_id(created.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("issued tenant key must be persisted");
            assert!(!stored.id.is_nil());
            assert_eq!(stored.user_id, Some(user.id));
            access_keys::Entity::revoke(&ctx.db, created.id)
                .await
                .unwrap();
        }
        let broad =
            access_keys::Entity::create_access_key(&ctx.db, tenant.workspace_id, Some(user.id))
                .await
                .unwrap();
        access_keys::Entity::revoke(&ctx.db, broad.id)
            .await
            .unwrap();
        let barrier = tokio::sync::Barrier::new(2);
        let issue = async {
            barrier.wait().await;
            if tenant_key {
                return access_keys::Entity::create_tenant_access_key(
                    &ctx.db,
                    tenant.tenant_id,
                    Some(user.id),
                    "membership-race",
                )
                .await
                .map(|key| key.plaintext);
            }
            access_keys::Entity::create_access_key(&ctx.db, tenant.workspace_id, Some(user.id))
                .await
                .map(|key| key.plaintext)
        };
        let remove = async {
            barrier.wait().await;
            tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, user.id).await
        };
        let (issued, removed) = tokio::join!(issue, remove);
        removed.unwrap();
        let keys = access_keys::Entity::find()
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
            access_keys::Entity::create_access_key(&ctx.db, tenant.workspace_id, Some(user.id))
                .await
                .is_err()
        );
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id)
            .await
            .unwrap();
        if let Ok(key) = issued {
            let response = request
                .get("/api/users")
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

fn bearer(key: &str) -> String {
    format!("Bearer {key}")
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one user is created, read, changed, signed in and deleted"
)]
async fn users_are_created_read_updated_and_deleted_by_id() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = bearer(&root_key);
        request
            .put(&format!("/api/users/{}/login-profile", tenant.owner_id))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"password":"must-not-replace-root-password"}))
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .delete(&format!("/api/users/{}", tenant.owner_id))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        let create = |body: serde_json::Value| {
            request
                .post("/api/users")
                .add_header("Authorization", &auth)
                .json(&body)
        };
        let password = "tenant-user-password";
        let created =
            create(serde_json::json!({"user_name":"Alice.Smith","password":password})).await;
        created.assert_status(StatusCode::CREATED);
        let body: serde_json::Value = created.json();
        assert!(body["email"].is_null());
        assert_eq!(body["user_name"], "Alice.Smith");
        assert_eq!(
            body["yrn"],
            format!("yrn:iam:{}:user/Alice.Smith", tenant.tenant_id)
        );
        let user_id = body["id"].as_str().unwrap().to_owned();

        for (name, status) in [
            ("alice.smith", StatusCode::CONFLICT),
            ("has space", StatusCode::UNPROCESSABLE_ENTITY),
            ("", StatusCode::UNPROCESSABLE_ENTITY),
            (&"n".repeat(65), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            create(serde_json::json!({"user_name":name,"password":password}))
                .await
                .assert_status(status);
        }
        create(serde_json::json!({"user_name":"short-password","password":"short"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        create(serde_json::json!({"user_name":"ok+=,.@_-name","password":password}))
            .await
            .assert_status(StatusCode::CREATED);

        let listed = request
            .get("/api/users")
            .add_header("Authorization", &auth)
            .await;
        listed.assert_status_ok();
        let listed = listed.json::<Vec<serde_json::Value>>();
        assert!(listed.iter().any(|user| user["id"] == user_id));
        assert!(
            listed
                .iter()
                .all(|user| user["id"] != tenant.owner_id.to_string()),
            "the tenant root is not listed"
        );
        let fetched = request
            .get(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await;
        fetched.assert_status_ok();
        assert_eq!(fetched.json::<serde_json::Value>()["id"], user_id);
        request
            .get(&format!("/api/users/{}", uuid::Uuid::new_v4()))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);

        let updated = request
            .patch(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"display_name":"Alice"}))
            .await;
        updated.assert_status_ok();
        assert_eq!(updated.json::<serde_json::Value>()["display_name"], "Alice");
        let cleared = request
            .patch(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"display_name":null}))
            .await;
        cleared.assert_status_ok();
        assert!(cleared.json::<serde_json::Value>()["display_name"].is_null());

        let login = |body: serde_json::Value| {
            request.post("/auth/tenant/login").json(&{
                let mut body = body;
                body["tenant_id"] = serde_json::json!(tenant.tenant_id);
                body
            })
        };
        let signed_in =
            login(serde_json::json!({"user_name":"ALICE.SMITH","password":password})).await;
        signed_in.assert_status(StatusCode::OK);
        let old_key = signed_in.json::<serde_json::Value>()["access_key"]
            .as_str()
            .unwrap()
            .to_owned();
        login(serde_json::json!({"user_name":"alice.smith","password":"wrong-password-here"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        request
            .put(&format!("/api/users/{user_id}/login-profile"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"password":"replacement-password"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/users")
            .add_header("Authorization", &bearer(&old_key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        login(serde_json::json!({"user_name":"alice.smith","password":password}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        login(serde_json::json!({"user_name":"alice.smith","password":"replacement-password"}))
            .await
            .assert_status(StatusCode::OK);

        // A live access key blocks the deletion until it is revoked.
        let refused = request
            .delete(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await;
        refused.assert_status(StatusCode::CONFLICT);
        assert!(refused.text().contains("access key"), "{}", refused.text());
        request
            .put(&format!("/api/users/{user_id}/login-profile"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"password":"another-replacement"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .delete(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);
        login(serde_json::json!({"user_name":"alice.smith","password":"another-replacement"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn a_user_with_groups_or_policies_cannot_be_deleted() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = bearer(&root_key);
        let user = request
            .post("/api/users")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"user_name":"bob","password":"tenant-user-password"}))
            .await;
        user.assert_status(StatusCode::CREATED);
        let user_id = user.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let group = request
            .post("/api/groups")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":"staff"}))
            .await;
        group.assert_status(StatusCode::CREATED);
        let group_id = group.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let membership = format!("/api/groups/{group_id}/members/{user_id}");
        request
            .put(&membership)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let refused = request
            .delete(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await;
        refused.assert_status(StatusCode::CONFLICT);
        assert!(
            refused.text().contains("group membership"),
            "{}",
            refused.text()
        );
        request
            .delete(&membership)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);

        let policy = crate::requests::fixtures::create_policy(
            &request,
            &auth,
            "readers",
            &crate::requests::fixtures::policy_document(
                "Allow",
                &["iam:ListGroups"],
                &["*".to_owned()],
            ),
        )
        .await;
        let attachment = format!("/api/users/{user_id}/policies/{policy}");
        request
            .put(&attachment)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let refused = request
            .delete(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await;
        refused.assert_status(StatusCode::CONFLICT);
        assert!(
            refused.text().contains("attached policy"),
            "{}",
            refused.text()
        );
        request
            .delete(&attachment)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .delete(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
    })
    .await;
}

#[tokio::test]
async fn removed_routes_are_gone() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        for path in [
            "/api/members".to_owned(),
            format!("/api/members/{}/removal-preview", tenant.owner_id),
            "/api/iam/groups".to_owned(),
        ] {
            request
                .get(&path)
                .add_header("Authorization", bearer(&root_key))
                .await
                .assert_status(StatusCode::NOT_FOUND);
        }
    })
    .await;
}
