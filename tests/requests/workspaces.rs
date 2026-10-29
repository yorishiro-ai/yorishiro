use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use super::{boot_request, boot_request_with_timezone};
use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::tenant_memberships::{self, MembershipRole};
use yorishiro::models::user_users;

/// A member of an existing tenant with a key of their own, created through the same model operations an invite redemption uses.
async fn member_key(
    ctx: &loco_rs::app::AppContext,
    tenant_id: uuid::Uuid,
    workspace_id: uuid::Uuid,
    role: MembershipRole,
) -> String {
    let email = format!("member-{}@example.com", uuid::Uuid::new_v4().simple());
    let member = user_users::create_user(&ctx.db, &email, "hunter2-hunter2", None)
        .await
        .expect("create member");
    tenant_memberships::add_member(&ctx.db, tenant_id, member.id, role)
        .await
        .expect("add member");
    super::fixtures::grant_workspace(
        ctx,
        workspace_id,
        member.id,
        yorishiro::models::workspace_memberships::WorkspaceRole::Member,
    )
    .await;
    issue_api_key(ctx, workspace_id, member.id, role.max_scope(), false).await
}

#[tokio::test]
async fn owner_can_create_list_view_and_delete_workspaces() {
    boot_request_with_timezone::<App, _, _>("America/New_York", |request, ctx| async move {
        let timezone = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap()
            .timezone
            .parse::<chrono_tz::Tz>()
            .unwrap();
        assert_ne!(
            timezone,
            chrono_tz::UTC,
            "test must boot with a non-default zone"
        );
        let args = TenantArgs::default();
        let initial_name = args.workspace_name.clone();
        let tenant = TestTenant::create(&request, &ctx, args).await;
        let tenant_id = tenant.tenant_id;
        let owner_id = tenant.owner_id;
        let request = tenant.authenticated(request);

        let response = request
            .post("/api/workspaces")
            .json(&serde_json::json!({ "name": "staging" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "staging");
        assert_eq!(body["tenant_id"], tenant_id.to_string());
        let staging_id = body["id"].as_str().unwrap().to_string();

        let response = request.get("/api/workspaces").await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        let names: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&initial_name.as_str()));
        assert!(names.contains(&"staging"));
        for workspace in body.as_array().unwrap() {
            let created_at =
                chrono::DateTime::parse_from_rfc3339(workspace["created_at"].as_str().unwrap())
                    .unwrap();
            let expected_offset = timezone
                .offset_from_utc_datetime(&created_at.naive_utc())
                .fix()
                .local_minus_utc();
            assert_eq!(created_at.offset().local_minus_utc(), expected_offset);
        }

        let staging_key = super::fixtures::issue_api_key(
            &ctx,
            staging_id.parse().unwrap(),
            owner_id,
            yorishiro::models::api_keys::ApiKeyScope::Read,
            false,
        )
        .await;
        let response = request
            .get(&format!("/api/workspaces/{staging_id}"))
            .add_header("Authorization", format!("Bearer {staging_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["entity_count"], 0);
        assert_eq!(body["relation_count"], 0);
        assert_eq!(body["schema_count"], 0);
        let created_at =
            chrono::DateTime::parse_from_rfc3339(body["created_at"].as_str().unwrap()).unwrap();
        let expected_offset = timezone
            .offset_from_utc_datetime(&created_at.naive_utc())
            .fix()
            .local_minus_utc();
        assert_eq!(created_at.offset().local_minus_utc(), expected_offset);

        let response = request
            .delete(&format!("/api/workspaces/{staging_id}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::NO_CONTENT);

        let response = request.get(&format!("/api/workspaces/{staging_id}")).await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn workspace_mutations_recheck_stale_administrator_contexts() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::{api_keys, tenancy, workspace_memberships::WorkspaceRole};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.unwrap();
        let admin = tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &super::fixtures::unique_email("workspace-stale-admin"),
            "hunter2-hunter2",
            None,
            MembershipRole::Admin,
        )
        .await
        .unwrap();
        yorishiro::models::workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            admin.id,
            WorkspaceRole::Member,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Read,
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
            scope: api_keys::ApiKeyScope::Read,
            audit: false,
        };
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Member)
            .await
            .unwrap();
        assert!(matches!(
            tenancy::create_workspace_as(&ctx.db, &actor, "must-not-create", None, None).await,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        assert!(matches!(
            tenancy::delete_workspace_as(&ctx.db, &actor, tenant.workspace_id).await,
            Err(yorishiro::YorishiroError::ScopeInsufficient { .. })
        ));
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, admin.id, MembershipRole::Admin)
            .await
            .unwrap();
        api_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        assert!(matches!(
            tenancy::create_workspace_as(&ctx.db, &actor, "must-not-create", None, None).await,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
        assert!(matches!(
            tenancy::delete_workspace_as(&ctx.db, &actor, tenant.workspace_id).await,
            Err(yorishiro::YorishiroError::Unauthenticated)
        ));
    })
    .await;
}

#[tokio::test]
async fn cannot_delete_a_tenants_only_workspace() {
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            workspace_id: main_id,
            api_key: owner_key,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .delete(&format!("/api/workspaces/{main_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::CONFLICT);
    })
    .await;
}

#[tokio::test]
async fn member_role_cannot_create_or_delete_workspaces() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id: main_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let member_key = member_key(&ctx, tenant_id, main_id, MembershipRole::Member).await;

        let response = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({ "name": "staging" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);

        let response = request
            .delete(&format!("/api/workspaces/{main_id}"))
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);

        // A Member-role key can still list/view workspaces, just not create/delete them.
        let response = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
    })
    .await;
}

#[tokio::test]
async fn workspaces_endpoints_require_authentication() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/workspaces").await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn workspace_administrators_manage_only_their_direct_grants() {
    use yorishiro::models::workspace_memberships::WorkspaceRole;
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let local = user_users::create_user(
            &ctx.db,
            &format!("local-{}@example.com", uuid::Uuid::new_v4()),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        let target = user_users::create_user(
            &ctx.db,
            &format!("target-{}@example.com", uuid::Uuid::new_v4()),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        for user_id in [local.id, target.id] {
            tenant_memberships::add_member(
                &ctx.db,
                tenant.tenant_id,
                user_id,
                MembershipRole::Member,
            )
            .await
            .unwrap();
        }
        let local_path = format!(
            "/api/workspaces/{}/members/{}",
            tenant.workspace_id, local.id
        );
        request
            .put(&local_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            local.id,
            yorishiro::models::api_keys::ApiKeyScope::Write,
            false,
        )
        .await;
        let path = format!(
            "/api/workspaces/{}/members/{}",
            tenant.workspace_id, target.id
        );
        request
            .put(&path)
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"role":"viewer"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        assert_eq!(
            yorishiro::models::workspace_memberships::Model::role(
                &ctx.db,
                tenant.workspace_id,
                target.id
            )
            .await
            .unwrap(),
            Some(WorkspaceRole::Viewer)
        );
        let target_key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            target.id,
            yorishiro::models::api_keys::ApiKeyScope::Read,
            false,
        )
        .await;
        request
            .put(&path)
            .add_header("Authorization", format!("Bearer {target_key}"))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            yorishiro::models::api_keys::ApiKeyScope::Migration,
            None,
            false,
        )
        .await
        .unwrap();
        request
            .put(&path)
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .delete(&format!(
                "/api/workspaces/{}/members/{}",
                tenant.workspace_id, tenant.owner_id
            ))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::OK);
        request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"name":"forbidden"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .post(&format!("/api/members/{}/role-preview", target.id))
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let sibling = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"name":format!("sibling-{}", Uuid::new_v4())}))
            .await;
        sibling.assert_status(StatusCode::CREATED);
        let sibling_id = sibling.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        request
            .put(&format!(
                "/api/workspaces/{sibling_id}/members/{}",
                target.id
            ))
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .delete(&path)
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {target_key}"))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        request
            .put(&local_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"role":"viewer"}))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .put(&path)
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"role":"admin"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn workspace_inventory_respects_direct_grants_and_service_binding() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let response = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"name":format!("private-sibling-{}", Uuid::new_v4())}))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let sibling = response.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        request
            .get(&format!("/api/workspaces/{sibling}"))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let member = member_key(
            &ctx,
            tenant.tenant_id,
            tenant.workspace_id,
            MembershipRole::Member,
        )
        .await;
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            yorishiro::models::api_keys::ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        for key in [&member, &service.plaintext] {
            let response = request
                .get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::OK);
            let workspaces = response.json::<serde_json::Value>();
            assert_eq!(workspaces.as_array().unwrap().len(), 1);
            assert_eq!(workspaces[0]["id"], tenant.workspace_id.to_string());
            let response = request
                .get(&format!("/api/workspaces/{sibling}"))
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
            let response = request
                .get(&format!("/api/workspaces/{}", tenant.workspace_id))
                .add_header("Authorization", format!("Bearer {key}"))
                .await;
            assert_eq!(response.status_code(), StatusCode::OK);
        }
    })
    .await;
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn workspace_endpoints_enforce_tenant_isolation() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            api_key: owner_a_key,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let TestTenant {
            workspace_id: workspace_b,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        // Tenant A's key must not be able to see or delete tenant B's workspace by guessing its id: workspace_workspaces has no RLS of its own, so get_workspace_in_tenant's explicit tenant_id check is the only thing enforcing this boundary.
        let response = request
            .get(&format!("/api/workspaces/{workspace_b}"))
            .add_header("Authorization", format!("Bearer {owner_a_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);

        let response = request
            .delete(&format!("/api/workspaces/{workspace_b}"))
            .add_header("Authorization", format!("Bearer {owner_a_key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    })
    .await;
}
