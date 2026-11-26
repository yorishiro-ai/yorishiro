use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{self, TenantArgs, TestTenant};

use super::*;

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
async fn call(request: &TestServer, session: &str, key: &str, tool: &str, args: Value) -> Value {
    call_tool(request, session, key, tool, args).await
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one set of tenant objects is created, read, changed and deleted through every IAM tool"
)]
async fn mcp_manages_users_groups_and_policies_by_id() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;

        let user = tool_result_json(
            &call(
                &request,
                &session,
                &key,
                "create_user",
                json!({"user_name":format!("mcp-{}", Uuid::new_v4().simple()),"password":"tenant-user-password"}),
            )
            .await,
        );
        let user_id = user["id"].as_str().unwrap().to_owned();
        assert!(user["yrn"].as_str().unwrap().contains(":user/mcp-"));
        let fetched = tool_result_json(&call(&request, &session, &key, "get_user", json!({"user_id":user_id})).await);
        assert_eq!(fetched["id"], user_id);
        let updated = tool_result_json(
            &call(&request, &session, &key, "update_user", json!({"user_id":user_id,"display_name":"MCP"})).await,
        );
        assert_eq!(updated["display_name"], "MCP");
        let listed = tool_result_json(&call(&request, &session, &key, "list_users", json!({})).await);
        assert!(listed.as_array().unwrap().iter().any(|u| u["id"] == user_id));
        let reset = call(&request, &session, &key, "update_login_profile", json!({"user_id":user_id,"password":"another-password"})).await;
        assert_eq!(tool_result_json(&reset)["updated"], true);

        let group = tool_result_json(&call(&request, &session, &key, "create_group", json!({"name":"mcp-group"})).await);
        let group_id = group["id"].as_str().unwrap().to_owned();
        let renamed = tool_result_json(&call(&request, &session, &key, "update_group", json!({"group_id":group_id,"name":"mcp-renamed"})).await);
        assert_eq!(renamed["name"], "mcp-renamed");
        call(&request, &session, &key, "add_user_to_group", json!({"group_id":group_id,"user_id":user_id})).await;
        let detail = tool_result_json(&call(&request, &session, &key, "get_group", json!({"group_id":group_id})).await);
        assert_eq!(detail["members"][0]["user_id"], user_id);
        let of_user = tool_result_json(&call(&request, &session, &key, "list_groups_for_user", json!({"user_id":user_id})).await);
        assert_eq!(of_user[0]["id"], group_id);

        let policy = tool_result_json(
            &call(
                &request,
                &session,
                &key,
                "create_policy",
                json!({"name":"mcp-policy","document":fixtures::policy_document("Allow", &["iam:ListGroups"], &["*".to_owned()])}),
            )
            .await,
        );
        let policy_id = policy["id"].as_str().unwrap().to_owned();
        let version = tool_result_json(
            &call(
                &request,
                &session,
                &key,
                "create_policy_version",
                json!({"policy_id":policy_id,"document":fixtures::policy_document("Allow", &["iam:ListUsers"], &["*".to_owned()]),"set_as_default":false}),
            )
            .await,
        );
        let version_id = version["id"].as_str().unwrap().to_owned();
        assert_eq!(version["version_number"], 2);
        let versions = tool_result_json(&call(&request, &session, &key, "list_policy_versions", json!({"policy_id":policy_id})).await);
        assert_eq!(versions.as_array().unwrap().len(), 2);
        let switched = tool_result_json(
            &call(&request, &session, &key, "set_default_policy_version", json!({"policy_id":policy_id,"version_id":version_id})).await,
        );
        assert_eq!(switched["is_default"], true);
        let read = tool_result_json(&call(&request, &session, &key, "get_policy_version", json!({"policy_id":policy_id,"version_id":version_id})).await);
        assert_eq!(read["id"], version_id);
        let got = tool_result_json(&call(&request, &session, &key, "get_policy", json!({"policy_id":policy_id})).await);
        assert_eq!(got["default_version_id"], version_id);
        let policies = tool_result_json(&call(&request, &session, &key, "list_policies", json!({})).await);
        assert_eq!(policies.as_array().unwrap().len(), 6);

        call(&request, &session, &key, "attach_group_policy", json!({"group_id":group_id,"policy_id":policy_id})).await;
        let attached = tool_result_json(&call(&request, &session, &key, "list_attached_group_policies", json!({"group_id":group_id})).await);
        assert_eq!(attached[0]["name"], "mcp-policy");
        call(&request, &session, &key, "attach_user_policy", json!({"user_id":user_id,"policy_id":policy_id})).await;
        let attached = tool_result_json(&call(&request, &session, &key, "list_attached_user_policies", json!({"user_id":user_id})).await);
        assert_eq!(attached[0]["policy_id"], policy_id);

        let simulated = tool_result_json(
            &call(&request, &session, &key, "simulate_principal_policy", json!({"user_id":user_id,"action":"iam:ListUsers"})).await,
        );
        assert_eq!(simulated["decision"], "allowed");
        assert_eq!(simulated["matched_policies"], json!(["mcp-policy"]));

        // Nothing is deleted while something still depends on it.
        for (tool, args) in [
            ("delete_user", json!({"user_id":user_id})),
            ("delete_group", json!({"group_id":group_id})),
            ("delete_policy", json!({"policy_id":policy_id})),
        ] {
            let refused = call(&request, &session, &key, tool, args).await;
            assert_eq!(refused["result"]["isError"], true, "{tool}: {refused}");
            assert_eq!(refused["result"]["structuredContent"]["error"]["code"], "conflict", "{tool}: {refused}");
        }
        call(&request, &session, &key, "detach_user_policy", json!({"user_id":user_id,"policy_id":policy_id})).await;
        call(&request, &session, &key, "detach_group_policy", json!({"group_id":group_id,"policy_id":policy_id})).await;
        call(&request, &session, &key, "remove_user_from_group", json!({"group_id":group_id,"user_id":user_id})).await;
        let first_version = policy["default_version_id"].as_str().unwrap();
        let refused = call(&request, &session, &key, "delete_policy_version", json!({"policy_id":policy_id,"version_id":version_id})).await;
        assert_eq!(refused["result"]["isError"], true, "the default version stays: {refused}");
        let removed = call(&request, &session, &key, "delete_policy_version", json!({"policy_id":policy_id,"version_id":first_version})).await;
        assert_eq!(tool_result_json(&removed)["deleted"], true);
        for (tool, args) in [
            ("delete_group", json!({"group_id":group_id})),
            ("delete_user", json!({"user_id":user_id})),
            ("delete_policy", json!({"policy_id":policy_id})),
        ] {
            let done = call(&request, &session, &key, tool, args).await;
            assert_eq!(tool_result_json(&done)["deleted"], true, "{tool}");
        }
    })
    .await;
}

#[tokio::test]
async fn removed_tools_are_not_listed_and_new_ones_are() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let listed = mcp_call(&request, &session, &key, 3, "tools/list", json!({})).await;
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        for gone in [
            "get_current_user",
            "update_current_user",
            "preview_member_removal",
            "preview_tenant_user_deletion",
            "list_members",
            "remove_member",
            "create_tenant_user",
            "reset_tenant_user_password",
            "list_iam_group_members",
            "update_iam_policy",
            "update_iam_group",
            "create_iam_group",
            "create_iam_policy",
            "list_iam_audit",
        ] {
            assert!(!names.contains(&gone), "{gone} is still listed");
        }
        for present in [
            "list_users",
            "create_user",
            "get_user",
            "update_user",
            "delete_user",
            "update_login_profile",
            "list_groups_for_user",
            "list_groups",
            "create_group",
            "get_group",
            "update_group",
            "delete_group",
            "add_user_to_group",
            "remove_user_from_group",
            "attach_group_policy",
            "detach_group_policy",
            "list_attached_group_policies",
            "list_policies",
            "create_policy",
            "get_policy",
            "delete_policy",
            "attach_user_policy",
            "detach_user_policy",
            "list_attached_user_policies",
            "create_policy_version",
            "get_policy_version",
            "list_policy_versions",
            "set_default_policy_version",
            "delete_policy_version",
            "simulate_principal_policy",
            "list_iam_audit_events",
        ] {
            assert!(names.contains(&present), "{present} is missing");
        }
    })
    .await;
}

#[tokio::test]
async fn service_keys_cannot_read_users() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let service = yorishiro::models::access_keys::Entity::create_access_key(
            &ctx.db,
            tenant.workspace_id,
            None,
        )
        .await
        .unwrap()
        .plaintext;
        let denied = call(
            &request,
            &session,
            &service,
            "get_user",
            json!({"user_id":tenant.owner_id}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true, "{denied}");
        request
            .get(&format!("/api/users/{}", tenant.owner_id))
            .add_header("Authorization", format!("Bearer {service}"))
            .await
            .assert_status_not_ok();
    })
    .await;
}
