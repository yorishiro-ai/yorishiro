use super::fixtures::{TenantArgs, TestTenant, issue_api_key};
use super::{boot_request, boot_request_with_timezone};
use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
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

        let response = request.get(&format!("/api/workspaces/{staging_id}")).await;
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
