use super::boot_request;
use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenant_memberships::MembershipRole;

#[tokio::test]
async fn invite_cancellation_and_acceptance_have_one_committed_outcome() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::{tenant_memberships, user_users, workspace_invites};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = user_users::create_user(
            &ctx.db, &super::fixtures::unique_email("cancel-accept-race"), "test-password", None,
        ).await.unwrap();
        let (invite, token) = workspace_invites::create_invite(
            &ctx.db, tenant.tenant_id, &user.email, MembershipRole::Viewer, chrono::Duration::hours(1),
        ).await.unwrap();
        let barrier = tokio::sync::Barrier::new(2);
        let cancel = async {
            barrier.wait().await;
            request.delete(&format!("/api/members/invites/{}", invite.id))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await
        };
        let accept = async {
            barrier.wait().await;
            request.post("/auth/invites/accept")
                .json(&serde_json::json!({"invite_token":token,"email":user.email,"password":"test-password"})).await
        };
        let (cancelled, accepted) = tokio::join!(cancel, accept);
        let membership = tenant_memberships::Entity::find()
            .filter(Column::TenantId.eq(tenant.tenant_id)).filter(Column::UserId.eq(user.id))
            .one(&ctx.db).await.unwrap();
        let stored = workspace_invites::Entity::find_by_id(invite.id).one(&ctx.db).await.unwrap().unwrap();
        match (cancelled.status_code(), accepted.status_code()) {
            (StatusCode::NO_CONTENT, StatusCode::UNPROCESSABLE_ENTITY) => {
                assert!(membership.is_none());
                assert!(stored.used_at.is_none());
                assert!(stored.expires_at <= chrono::Utc::now());
            }
            (StatusCode::CONFLICT, StatusCode::OK) => {
                assert_eq!(membership.unwrap().role, MembershipRole::Viewer.as_db_str());
                assert!(stored.used_at.is_some());
            }
            _ => panic!("unexpected cancellation/acceptance: {} {} / {} {}",
                cancelled.status_code(), cancelled.text(), accepted.status_code(), accepted.text()),
        }
    }).await;
}

#[tokio::test]
async fn non_administrative_credentials_cannot_manage_invites_or_preview_members() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::{api_keys, tenant_memberships, user_users, workspace_invites};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let (invite, _) = workspace_invites::create_invite(
            &ctx.db, tenant.tenant_id, &super::fixtures::unique_email("denied-invite"),
            MembershipRole::Member, chrono::Duration::hours(1),
        ).await.unwrap();
        let mut credentials = Vec::new();
        for role in [MembershipRole::Member, MembershipRole::Viewer] {
            let user = user_users::create_user(&ctx.db, &super::fixtures::unique_email("denied-member"), "test-password", None).await.unwrap();
            tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id, role).await.unwrap();
            super::fixtures::grant_workspace(&ctx, tenant.workspace_id, user.id,
                yorishiro::models::workspace_memberships::WorkspaceRole::Admin).await;
            credentials.push((issue_api_key(&ctx, tenant.workspace_id, user.id, role.max_scope(), false).await, StatusCode::FORBIDDEN));
        }
        let service = api_keys::Entity::create_api_key(&ctx.db, tenant.workspace_id, ApiKeyScope::Migration, None, false).await.unwrap();
        credentials.push((service.plaintext, StatusCode::UNAUTHORIZED));
        for (key, expected) in credentials {
            let authorization = format!("Bearer {key}");
            let response = request.get("/api/members/invites").add_header("Authorization", authorization.clone()).await;
            assert_eq!(response.status_code(), expected);
            let response = request.post("/api/members/invites").add_header("Authorization", authorization.clone())
                .json(&serde_json::json!({"email":"denied@example.com","role":"owner","expires_at":chrono::Utc::now() + chrono::Duration::hours(1)})).await;
            assert_eq!(response.status_code(), expected);
            let response = request.delete(&format!("/api/members/invites/{}", invite.id)).add_header("Authorization", authorization.clone()).await;
            assert_eq!(response.status_code(), expected);
            let response = request.post(&format!("/api/members/invites/{}/reissue", invite.id)).add_header("Authorization", authorization.clone())
                .json(&serde_json::json!({"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)})).await;
            assert_eq!(response.status_code(), expected);
            let response = request.get(&format!("/api/members/{}/removal-preview", tenant.owner_id)).add_header("Authorization", authorization.clone()).await;
            assert_eq!(response.status_code(), expected);
            let response = request.post(&format!("/api/members/{}/role-preview", tenant.owner_id)).add_header("Authorization", authorization.clone())
                .json(&serde_json::json!({"role":"viewer"})).await;
            assert_eq!(response.status_code(), expected);
            let response = request.post("/api/members/transfer-ownership").add_header("Authorization", authorization)
                .json(&serde_json::json!({"successor_user_id":tenant.owner_id})).await;
            assert_eq!(response.status_code(), expected);
        }
        assert_eq!(workspace_invites::Entity::find_by_id(invite.id).one(&ctx.db).await.unwrap().unwrap(), invite);
        let owner = tenant_memberships::Entity::find()
            .filter(Column::TenantId.eq(tenant.tenant_id))
            .filter(Column::UserId.eq(tenant.owner_id))
            .one(&ctx.db).await.unwrap().expect("owner membership must remain");
        assert_eq!(owner.role, MembershipRole::Owner.as_db_str());
    }).await;
}

#[tokio::test]
async fn administrators_cannot_grant_or_remove_owner_authority() {
    use yorishiro::models::api_keys::ApiKeyScope;
    use yorishiro::models::{tenant_memberships, user_users};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let admin = user_users::create_user(&ctx.db, &super::fixtures::unique_email("owner-boundary"), "test-password", None).await.unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Admin).await.unwrap();
        let key = super::fixtures::issue_api_key(&ctx, tenant.workspace_id, admin.id, ApiKeyScope::Migration, false).await;
        let auth = format!("Bearer {key}");
        request.post("/api/members").add_header("Authorization", &auth)
            .json(&serde_json::json!({"email":admin.email,"role":"owner"})).await.assert_status(StatusCode::FORBIDDEN);
        request.delete(&format!("/api/members/{}", tenant.owner_id)).add_header("Authorization", &auth)
            .await.assert_status(StatusCode::FORBIDDEN);
        let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
        request.post("/api/members/invites").add_header("Authorization", &auth)
            .json(&serde_json::json!({"email":"other@example.com","role":"owner","expires_at":expiry})).await.assert_status(StatusCode::FORBIDDEN);
        let (invite, _) = yorishiro::models::workspace_invites::create_invite(&ctx.db, tenant.tenant_id, "owner@example.com", MembershipRole::Owner, chrono::Duration::hours(1)).await.unwrap();
        request.post(&format!("/api/members/invites/{}/reissue", invite.id)).add_header("Authorization", &auth)
            .json(&serde_json::json!({"expires_at":expiry})).await.assert_status(StatusCode::FORBIDDEN);
        request.post("/api/members/invites").add_header("Authorization", &auth)
            .json(&serde_json::json!({"email":"member@example.com","role":"member","expires_at":expiry})).await.assert_status(StatusCode::CREATED);
    }).await;
}

#[tokio::test]
async fn owner_can_transfer_to_existing_member_but_admin_cannot() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::_entities::tenant_memberships::Column;
    use yorishiro::models::{tenant_memberships, user_users};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let successor = user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("successor"),
            "test-password",
            None,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(
            &ctx.db,
            tenant.tenant_id,
            successor.id,
            MembershipRole::Viewer,
        )
        .await
        .unwrap();
        let path = "/api/members/transfer-ownership";
        let missing = request
            .post(path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"successor_user_id":uuid::Uuid::new_v4()}))
            .await;
        assert_eq!(missing.status_code(), StatusCode::NOT_FOUND);
        let response = request
            .post(path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"successor_user_id":successor.id}))
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::NO_CONTENT,
            "{}",
            response.text()
        );
        for (user_id, role) in [(tenant.owner_id, "admin"), (successor.id, "owner")] {
            let membership = tenant_memberships::Entity::find()
                .filter(Column::TenantId.eq(tenant.tenant_id))
                .filter(Column::UserId.eq(user_id))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(membership.role, role);
        }
        let forbidden = request
            .post(path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"successor_user_id":successor.id}))
            .await;
        assert_eq!(forbidden.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn invite_cancellation_and_reissue_invalidate_old_tokens() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let email = super::fixtures::unique_email("rotated-invite");
        let (invite, old_token) = yorishiro::models::workspace_invites::create_invite(
            &ctx.db,
            tenant.tenant_id,
            &email,
            MembershipRole::Member,
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();
        let path = format!("/api/members/invites/{}", invite.id);
        for _ in 0..2 {
            let cancelled = request
                .delete(&path)
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .await;
            assert_eq!(cancelled.status_code(), StatusCode::NO_CONTENT);
        }
        let rejected = request
            .post("/auth/signup")
            .json(&serde_json::json!({"invite_token":old_token,"password":"test-password"}))
            .await;
        assert_eq!(rejected.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        let rotated = request
            .post(&format!("{path}/reissue"))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(
                &serde_json::json!({"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}),
            )
            .await;
        assert_eq!(rotated.status_code(), StatusCode::OK, "{}", rotated.text());
        let rotated = rotated.json::<serde_json::Value>();
        let token = rotated["token"].as_str().unwrap();
        assert_ne!(token, old_token);
        let stale = request
            .post("/auth/signup")
            .json(&serde_json::json!({"invite_token":old_token,"password":"test-password"}))
            .await;
        assert_eq!(stale.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        let accepted = request
            .post("/auth/signup")
            .json(&serde_json::json!({"invite_token":token,"password":"test-password"}))
            .await;
        assert_eq!(
            accepted.status_code(),
            StatusCode::CREATED,
            "{}",
            accepted.text()
        );
        let used = request
            .post(&format!("{path}/reissue"))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(
                &serde_json::json!({"expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}),
            )
            .await;
        assert_eq!(used.status_code(), StatusCode::CONFLICT);
    })
    .await;
}

#[tokio::test]
async fn administrator_can_issue_and_list_secret_free_invites() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let email = super::fixtures::unique_email("managed-invite");
        let body = serde_json::json!({"email":email,"role":"member","expires_at":chrono::Utc::now() + chrono::Duration::hours(1)});
        let anonymous = request.post("/api/members/invites").json(&body).await;
        assert_eq!(anonymous.status_code(), StatusCode::UNAUTHORIZED);
        let created = request.post("/api/members/invites")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&body).await;
        assert_eq!(created.status_code(), StatusCode::CREATED, "{}", created.text());
        let created = created.json::<serde_json::Value>();
        let token = created["token"].as_str().unwrap();
        assert!(!token.is_empty());
        assert!(created.get("token_hash").is_none());
        let listed = request.get("/api/members/invites")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let records = listed.json::<serde_json::Value>();
        let record = records.as_array().unwrap().iter().find(|record| record["id"] == created["id"]).unwrap();
        assert_eq!(record["email"], email);
        assert!(record.get("token").is_none());
        assert!(!records.to_string().contains(token));
        assert!(!records.to_string().contains("token_hash"));
        let accepted = request.post("/auth/signup")
            .json(&serde_json::json!({"invite_token":token,"password":"test-password"})).await;
        assert_eq!(accepted.status_code(), StatusCode::CREATED, "{}", accepted.text());
    }).await;
}

#[tokio::test]
async fn membership_mutations_reject_stale_admin_contexts() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::{api_keys, tenant_memberships};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let admin = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("stale-admin"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Admin)
            .await
            .unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            Some(admin.id),
            false,
        )
        .await
        .unwrap();
        let actor = api_keys::AuthContext {
            api_key_id: key.id,
            workspace_id: tenant.workspace_id,
            tenant_id: tenant.tenant_id,
            user_id: Some(admin.id),
            scope: ApiKeyScope::Read,
            audit: false,
        };
        let target = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Member)
            .await
            .unwrap();
        let result = tenant_memberships::remove_member_as(&ctx.db, &actor, admin.id).await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        let result = api_keys::Entity::revoke_for_tenant(&ctx.db, &actor, target.id).await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        let result = tenant_memberships::update_member_access(
            &ctx.db,
            &actor,
            admin.id,
            MembershipRole::Admin,
        )
        .await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Admin)
            .await
            .unwrap();
        api_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        let result = api_keys::Entity::revoke_for_tenant(&ctx.db, &actor, target.id).await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
        assert!(
            api_keys::Entity::find_by_id(target.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
        let result = tenant_memberships::remove_member_as(&ctx.db, &actor, admin.id).await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
        let result = tenant_memberships::update_member_access(
            &ctx.db,
            &actor,
            admin.id,
            MembershipRole::Owner,
        )
        .await;
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
        assert!(
            tenant_memberships::Entity::find()
                .filter(
                    yorishiro::models::_entities::tenant_memberships::Column::TenantId
                        .eq(tenant.tenant_id)
                )
                .filter(
                    yorishiro::models::_entities::tenant_memberships::Column::UserId.eq(admin.id)
                )
                .one(&ctx.db)
                .await
                .unwrap()
                .is_some()
        );
    })
    .await;
}

#[tokio::test]
async fn role_preview_reports_lost_workspaces_without_keys() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = yorishiro::models::user_users::create_user(
            &ctx.db,
            &super::fixtures::unique_email("preview-no-keys"),
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
        for role in ["member", "viewer"] {
            let response = request
                .post(&format!("/api/members/{}/role-preview", user.id))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&serde_json::json!({"role": role}))
                .await;
            assert_eq!(response.status_code(), StatusCode::OK);
            let preview = response.json::<serde_json::Value>();
            assert_eq!(preview["keys"], serde_json::json!([]));
            assert_eq!(
                preview["lost_workspace_ids"],
                serde_json::json!([tenant.workspace_id])
            );
        }
        super::fixtures::grant_workspace(
            &ctx,
            tenant.workspace_id,
            user.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Viewer,
        )
        .await;
        let response = request
            .post(&format!("/api/members/{}/role-preview", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"role": "viewer"}))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>()["lost_workspace_ids"],
            serde_json::json!([])
        );
    })
    .await;
}

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
            super::fixtures::grant_workspace(
                &ctx,
                tenant.workspace_id,
                user.id,
                yorishiro::models::workspace_memberships::WorkspaceRole::Admin,
            )
            .await;
            let preview = request
                .post(&format!("/api/members/{}/role-preview", user.id))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&serde_json::json!({"role":role}))
                .await;
            assert_eq!(preview.status_code(), StatusCode::OK);
            let preview = preview.json::<serde_json::Value>();
            assert_eq!(preview["blocked_by_last_owner"], false);
            let response = request
                .post("/api/members")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&serde_json::json!({"email":user.email,"role":role}))
                .await;
            assert_eq!(response.status_code(), StatusCode::CREATED);
            let access = response.json::<serde_json::Value>();
            assert_eq!(preview["keys"], access["keys"]);
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
        let preview = request
            .get(&format!("/api/members/{}/removal-preview", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(preview.status_code(), StatusCode::OK);
        let preview = preview.json::<serde_json::Value>();
        assert_eq!(preview["blocked_by_last_owner"], false);
        assert!(
            preview["lost_workspace_ids"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(tenant.workspace_id))
        );
        assert_eq!(preview["revoked_api_key_ids"].as_array().unwrap().len(), 2);
        let still_valid = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {broad}"))
            .await;
        assert_eq!(still_valid.status_code(), StatusCode::OK);
        let removed = request
            .delete(&format!("/api/members/{}", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(removed.status_code(), StatusCode::NO_CONTENT);
        let preview = request
            .get(&format!("/api/members/{}/removal-preview", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(preview.status_code(), StatusCode::OK);
        let preview = preview.json::<serde_json::Value>();
        assert!(preview["role"].is_null());
        assert_eq!(preview["revoked_api_key_ids"], serde_json::json!([]));
        assert_eq!(preview["lost_workspace_ids"], serde_json::json!([]));
        let repeated = request
            .delete(&format!("/api/members/{}", user.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(repeated.status_code(), StatusCode::NO_CONTENT);
        let missing = request
            .delete(&format!("/api/members/{}", uuid::Uuid::new_v4()))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(missing.status_code(), StatusCode::NO_CONTENT);
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
        super::fixtures::grant_workspace(
            &ctx,
            workspace_id,
            member.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Member,
        )
        .await;
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
        let preview = request
            .get(&format!("/api/members/{}/removal-preview", tenant.owner_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        assert_eq!(preview.status_code(), StatusCode::OK);
        assert_eq!(
            preview.json::<serde_json::Value>()["blocked_by_last_owner"],
            true
        );
        let role_preview = request
            .post(&format!("/api/members/{}/role-preview", tenant.owner_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"role":"admin"}))
            .await;
        assert_eq!(role_preview.status_code(), StatusCode::OK);
        assert_eq!(
            role_preview.json::<serde_json::Value>()["blocked_by_last_owner"],
            true
        );
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
        super::fixtures::grant_workspace(
            &ctx,
            tenant.workspace_id,
            user.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Viewer,
        )
        .await;
        #[cfg(feature = "enterprise")]
        if tenant_key {
            let created = yorishiro::edition::ee::models::api_keys::create_tenant_api_key(
                &ctx.db,
                tenant.tenant_id,
                "read",
                Some(user.id),
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

#[cfg(feature = "enterprise")]
#[tokio::test]
async fn tenant_key_issuance_cannot_survive_concurrent_membership_removal() {
    issuance_and_removal_are_serialized(true).await;
}
