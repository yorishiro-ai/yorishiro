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
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            user.id,
            MembershipRole::Admin,
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
        let removed = request
            .delete(&format!("/api/members/{}", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(removed.status_code(), StatusCode::NO_CONTENT);
        for key in [&broad, &narrow] {
            let response = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
        }
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            user.id,
            MembershipRole::Admin,
        )
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
            issue_api_key(&ctx, workspace_id, member.id, ApiKeyScope::Write, false).await;

        let response = request
            .get("/api/members")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
        let response = request
            .delete(&format!("/api/members/{}", member.id))
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
        let response = request
            .delete(&format!("/api/members/{}", uuid::Uuid::nil()))
            .await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn last_owner_cannot_be_demoted_or_removed_through_any_membership_operation() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let args = TenantArgs::default();
        let email = args.owner_email.clone();
        let tenant = TestTenant::create(&request, &ctx, args).await;
        let demoted = request
            .post("/api/members")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"email":email,"role":"admin"}))
            .await;
        assert_eq!(demoted.status_code(), StatusCode::CONFLICT);
        let removed = request
            .delete(&format!("/api/members/{}", tenant.owner_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(removed.status_code(), StatusCode::CONFLICT);
        let demoted = yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            tenant.owner_id,
            MembershipRole::Viewer,
        )
        .await;
        assert!(matches!(
            demoted,
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));
        let identity = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(identity.status_code(), StatusCode::OK);
    })
    .await;
}

#[tokio::test]
async fn competing_owner_demotion_and_removal_preserve_an_owner() {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::tenant_memberships;

    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let second = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("competing-owner"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, second.id, MembershipRole::Owner)
            .await
            .unwrap();
        let barrier = tokio::sync::Barrier::new(2);
        let demote = async {
            barrier.wait().await;
            tenant_memberships::add_member(
                &ctx.db,
                tenant.tenant_id,
                tenant.owner_id,
                MembershipRole::Admin,
            )
            .await
        };
        let remove = async {
            barrier.wait().await;
            tenant_memberships::remove_member(&ctx.db, tenant.tenant_id, second.id).await
        };
        let (demoted, removed) = tokio::join!(demote, remove);
        assert_ne!(
            demoted.is_ok(),
            removed.is_ok(),
            "exactly one owner change may commit: {demoted:?}, {removed:?}"
        );
        let rejected = if demoted.is_err() { demoted } else { removed };
        assert!(matches!(
            rejected,
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));
        let owners = tenant_memberships::Entity::find()
            .filter(Column::TenantId.eq(tenant.tenant_id))
            .filter(Column::Role.eq(MembershipRole::Owner.as_db_str()))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(owners, 1);
    })
    .await;
}

async fn issuance_and_removal_are_serialized(tenant_key: bool) {
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
    use yorishiro::models::_entities::api_keys::Column;
    use yorishiro::models::{api_keys, tenant_memberships};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("issuance-race"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id, MembershipRole::Viewer)
            .await
            .unwrap();
        assert!(
            api_keys::Entity::create_api_key(
                &ctx.db,
                tenant.workspace_id,
                ApiKeyScope::Write,
                Some(user.id),
                false
            )
            .await
            .is_err()
        );
        let barrier = tokio::sync::Barrier::new(2);
        let issue = async {
            barrier.wait().await;
            if tenant_key {
                #[cfg(feature = "enterprise")]
                {
                    return yorishiro::edition::ee::models::api_keys::create_tenant_api_key(
                        &ctx.db,
                        tenant.tenant_id,
                        "read",
                        Some(user.id),
                    )
                    .await
                    .map(|key| key.plaintext);
                }
                #[cfg(not(feature = "enterprise"))]
                unreachable!("tenant keys require the overlay");
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
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(
            keys, 0,
            "removal must delete a racing issuance or prevent its insertion"
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
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id, MembershipRole::Viewer)
            .await
            .unwrap();
        if let Ok(key) = issued {
            let response = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
        } else {
            assert!(matches!(
                issued,
                Err(yorishiro::YorishiroError::Unauthenticated)
            ));
        }
    })
    .await;
}

#[tokio::test]
async fn workspace_key_issuance_cannot_survive_concurrent_membership_removal() {
    issuance_and_removal_are_serialized(false).await;
}

#[cfg(feature = "enterprise")]
#[tokio::test]
async fn tenant_key_issuance_cannot_survive_concurrent_membership_removal() {
    issuance_and_removal_are_serialized(true).await;
}
