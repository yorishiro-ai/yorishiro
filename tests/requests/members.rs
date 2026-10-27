use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;

#[tokio::test]
async fn existing_keys_follow_current_role_without_widening_their_grant() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("role-change"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        let broad = issue_api_key(
            &ctx,
            tenant.workspace_id,
            user.id,
            ApiKeyScope::Migration,
            false,
        )
        .await;
        let narrow =
            issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        let response = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {broad}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
        for (role, expected) in [
            (MembershipRole::Admin, "migration"),
            (MembershipRole::Viewer, "read"),
            (MembershipRole::Member, "write"),
            (MembershipRole::Owner, "migration"),
        ] {
            let response = request
                .post("/api/members")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&serde_json::json!({"email":user.email,"role":role}))
                .await;
            assert_eq!(response.status_code(), StatusCode::CREATED);
            let access = response.json::<serde_json::Value>();
            let keys = access["keys"].as_array().unwrap();
            assert_eq!(keys.len(), 2);
            assert!(keys.iter().any(
                |key| key["stored_scope"] == "migration" && key["effective_scope"] == expected
            ));
            assert!(
                keys.iter()
                    .any(|key| key["stored_scope"] == "read" && key["effective_scope"] == "read")
            );
            for key in keys {
                assert!(key["api_key_id"].as_str().is_some());
                assert!(key.get("key_hash").is_none());
                assert!(key.get("plaintext").is_none());
            }
            let login = request
                .post("/auth/login")
                .json(&serde_json::json!({
                    "email": user.email,
                    "password": "hunter2-hunter2",
                    "workspace_id": tenant.workspace_id,
                }))
                .await;
            assert_eq!(login.status_code(), StatusCode::OK);
            let login = login.json::<serde_json::Value>();
            assert_eq!(login["scope"], expected);
            let fresh_key = login["api_key"].as_str().unwrap();
            let fresh = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {fresh_key}"))
                .await;
            assert_eq!(fresh.status_code(), StatusCode::OK);
            assert_eq!(fresh.json::<serde_json::Value>()["scope"], expected);
            for (key, scope) in [(&broad, expected), (&narrow, "read")] {
                let response = request
                    .get("/api/whoami")
                    .add_header("Authorization", format!("Bearer {key}"))
                    .await;
                assert_eq!(response.status_code(), StatusCode::OK);
                assert_eq!(response.json::<serde_json::Value>()["scope"], scope);
            }
            yorishiro::models::api_keys::Entity::revoke(
                &ctx.db,
                login["api_key_id"].as_str().unwrap().parse().unwrap(),
            )
            .await
            .unwrap();
            let revoked = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {fresh_key}"))
                .await;
            assert_eq!(revoked.status_code(), StatusCode::UNAUTHORIZED);
        }
        yorishiro::models::tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, user.id)
            .await
            .unwrap();
        for key in [&broad, &narrow] {
            let response = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
        }
    })
    .await;
}

#[tokio::test]
async fn owner_can_list_and_add_members() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let args = TenantArgs::default();
        let owner_email = args.owner_email.clone();
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, args).await;

        // The invitee must already have an account before they can be added by email.
        let invitee = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("invitee"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create invitee");

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": invitee.email,
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let body: serde_json::Value = response.json();
        assert_eq!(body["user_id"], invitee.id.to_string());
        assert_eq!(body["role"], "member");

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        let emails: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["email"].as_str().unwrap())
            .collect();
        assert!(emails.contains(&owner_email.as_str()));
        assert!(emails.contains(&invitee.email.as_str()));
    })
    .await;
}

#[tokio::test]
async fn add_member_rejects_an_email_with_no_account() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({
                "email": super::fixtures::unique_email("nobody"),
                "role": "member",
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn member_role_cannot_manage_members() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("member"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant_id,
            member.id,
            MembershipRole::Member,
        )
        .await
        .expect("add member");
        let member_key =
            issue_api_key(&ctx, workspace_id, member.id, ApiKeyScope::Migration, false).await;

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn members_endpoints_require_authentication() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/members").await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}
