use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{self, TenantArgs, TestTenant};

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
async fn create_user(request: &loco_rs::TestServer, auth: &str, name: &str) -> String {
    let response = request
        .post("/api/users")
        .add_header("Authorization", auth)
        .json(&json!({"user_name":name,"password":"tenant-user-password"}))
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<Value>()["user_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one group history covers names, membership, policies, conflicts and deletion"
)]
async fn groups_have_case_insensitive_names_members_and_policies() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {key}");
        let created = request
            .post("/api/groups")
            .add_header("Authorization", &auth)
            .json(&json!({"name":"Operators"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let group = created.json::<Value>();
        assert_eq!(
            group["yrn"],
            format!("yrn:iam:{}:group/Operators", tenant.tenant_id)
        );
        let group_id = group["group_id"].as_str().unwrap().to_owned();
        for (name, status) in [
            ("operators", StatusCode::CONFLICT),
            ("OPERATORS", StatusCode::CONFLICT),
            ("bad name", StatusCode::UNPROCESSABLE_ENTITY),
            (" ", StatusCode::UNPROCESSABLE_ENTITY),
            (&"g".repeat(129), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            request
                .post("/api/groups")
                .add_header("Authorization", &auth)
                .json(&json!({"name":name}))
                .await
                .assert_status(status);
        }
        request
            .post("/api/groups")
            .add_header("Authorization", &auth)
            .json(&json!({"name":"g".repeat(128)}))
            .await
            .assert_status(StatusCode::CREATED);
        request
            .post("/api/groups")
            .json(&json!({"name":"unauthenticated"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let user_id = create_user(&request, &auth, "carol").await;
        let membership = format!("/api/groups/{group_id}/members/{user_id}");
        request
            .put(&membership)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .put(&membership)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .put(&format!(
                "/api/groups/{group_id}/members/{}",
                uuid::Uuid::new_v4()
            ))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);

        let detail = request
            .get(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .await;
        detail.assert_status_ok();
        let detail = detail.json::<Value>();
        assert_eq!(detail["name"], "Operators");
        assert_eq!(detail["members"][0]["user_id"], user_id);
        assert_eq!(
            detail["members"][0]["yrn"],
            format!("yrn:iam:{}:user/carol", tenant.tenant_id)
        );
        request
            .get(&format!("/api/groups/{group_id}/members"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let groups_of_user = request
            .get(&format!("/api/users/{user_id}/groups"))
            .add_header("Authorization", &auth)
            .await;
        groups_of_user.assert_status_ok();
        assert_eq!(groups_of_user.json::<Value>()[0]["group_id"], group_id);

        let refused = request
            .delete(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .await;
        refused.assert_status(StatusCode::CONFLICT);
        assert!(refused.text().contains("member"), "{}", refused.text());

        let policy = fixtures::create_policy(
            &request,
            &auth,
            "list-groups",
            &fixtures::policy_document("Allow", &["iam:ListGroups"], &["*".to_owned()]),
        )
        .await;
        let attachment = format!("/api/groups/{group_id}/policies/{policy}");
        request
            .put(&attachment)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .put(&attachment)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        let attached = request
            .get(&format!("/api/groups/{group_id}/policies"))
            .add_header("Authorization", &auth)
            .await;
        attached.assert_status_ok();
        let attached = attached.json::<Vec<Value>>();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0]["name"], "list-groups");
        assert_eq!(
            attached[0]["yrn"],
            format!("yrn:iam:{}:policy/list-groups", tenant.tenant_id)
        );

        // The member inherits the group's policy.
        let member_key =
            fixtures::issue_tenant_key(&ctx, tenant.tenant_id, user_id.parse().unwrap()).await;
        request
            .get("/api/groups")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await
            .assert_status_ok();
        request
            .get("/api/users")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);

        request
            .delete(&membership)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let refused = request
            .delete(&format!("/api/groups/{group_id}"))
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

        let renamed = request
            .patch(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .json(&json!({"name":"Renamed"}))
            .await;
        renamed.assert_status_ok();
        assert_eq!(renamed.json::<Value>()["name"], "Renamed");
        request
            .patch(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .json(&json!({"name":"G".repeat(128)}))
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .delete(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get(&format!("/api/groups/{group_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn group_listings_page_by_limit_and_offset() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {key}");
        for name in ["alpha", "bravo", "charlie"] {
            request
                .post("/api/groups")
                .add_header("Authorization", &auth)
                .json(&json!({"name":name}))
                .await
                .assert_status(StatusCode::CREATED);
        }
        let all = request
            .get("/api/groups")
            .add_header("Authorization", &auth)
            .await
            .json::<Vec<Value>>();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|g| g["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["alpha", "bravo", "charlie"]);
        let page = request
            .get("/api/groups?limit=1&offset=1")
            .add_header("Authorization", &auth)
            .await
            .json::<Vec<Value>>();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["name"], "bravo");
    })
    .await;
}
