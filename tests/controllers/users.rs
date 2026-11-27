use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use yorishiro::App;
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
        let user_id = body["user_id"].as_str().unwrap().to_owned();

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
        assert!(listed.iter().any(|user| user["user_id"] == user_id));
        assert!(
            listed
                .iter()
                .all(|user| user["user_id"] != tenant.owner_id.to_string()),
            "the tenant root is not listed"
        );
        let fetched = request
            .get(&format!("/api/users/{user_id}"))
            .add_header("Authorization", &auth)
            .await;
        fetched.assert_status_ok();
        assert_eq!(fetched.json::<serde_json::Value>()["user_id"], user_id);
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
async fn deleting_a_user_removes_iam_associations_and_revokes_keys() {
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
        let user_id = user.json::<serde_json::Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let group = request
            .post("/api/groups")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":"staff"}))
            .await;
        group.assert_status(StatusCode::CREATED);
        let group_id = group.json::<serde_json::Value>()["group_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let membership = format!("/api/groups/{group_id}/members/{user_id}");
        request
            .put(&membership)
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
        let key = crate::requests::fixtures::issue_tenant_key(
            &ctx,
            tenant.tenant_id,
            user_id.parse().unwrap(),
        )
        .await;
        let user_path = format!("/api/users/{user_id}");
        request
            .delete(&user_path)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get(&user_path)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);
        request
            .get("/api/caller-identity")
            .add_header("Authorization", bearer(&key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let group = request
            .get(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .await;
        group.assert_status(StatusCode::OK);
        assert_eq!(
            group.json::<serde_json::Value>()["members"],
            serde_json::json!([])
        );
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
