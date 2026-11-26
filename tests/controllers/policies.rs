use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::App;

use crate::requests::boot_request;
use crate::requests::fixtures::{self, TenantArgs, TestTenant};

fn allow(action: &str) -> Value {
    fixtures::policy_document("Allow", &[action], &["*".to_owned()])
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one policy is versioned and evaluated through its whole life"
)]
async fn policies_are_named_and_versioned_and_evaluate_their_default_version() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {key}");
        let created = request
            .post("/api/policies")
            .add_header("Authorization", &auth)
            .json(&json!({"name":"group-lister","document":allow("iam:ListGroups")}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let policy = created.json::<Value>();
        assert_eq!(
            policy["yrn"],
            format!("yrn:iam:{}:policy/group-lister", tenant.tenant_id)
        );
        assert_eq!(policy["managed"], false);
        let policy_id = policy["id"].as_str().unwrap().to_owned();
        let first_version = policy["default_version_id"].as_str().unwrap().to_owned();

        for (name, document, status) in [
            ("group-lister", allow("iam:ListGroups"), StatusCode::CONFLICT),
            ("AdministratorAccess", allow("iam:ListGroups"), StatusCode::CONFLICT),
            ("bad name", allow("iam:ListGroups"), StatusCode::UNPROCESSABLE_ENTITY),
            ("old-format", json!({"statements":[]}), StatusCode::UNPROCESSABLE_ENTITY),
            ("unknown-action", allow("iam:Nope"), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            request
                .post("/api/policies")
                .add_header("Authorization", &auth)
                .json(&json!({"name":name,"document":document}))
                .await
                .assert_status(status);
        }
        request
            .put(&format!("/api/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .json(&json!({"name":"renamed","document":allow("iam:ListGroups")}))
            .await
            .assert_status(StatusCode::METHOD_NOT_ALLOWED);

        let user = request
            .post("/api/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":"dave","password":"tenant-user-password"}))
            .await
            .json::<Value>();
        let user_id = user["id"].as_str().unwrap().to_owned();
        request
            .put(&format!("/api/users/{user_id}/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let user_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, user_id.parse().unwrap()).await;
        let member = format!("Bearer {user_key}");
        let can_list_groups = || async {
            request
                .get("/api/groups")
                .add_header("Authorization", &member)
                .await
                .status_code()
        };
        assert_eq!(can_list_groups().await, StatusCode::OK);

        // A new version does not change evaluation until it becomes the default.
        let second = request
            .post(&format!("/api/policies/{policy_id}/versions"))
            .add_header("Authorization", &auth)
            .json(&json!({"document":fixtures::policy_document("Deny", &["iam:ListGroups"], &["*".to_owned()])}))
            .await;
        second.assert_status(StatusCode::CREATED);
        let second = second.json::<Value>();
        assert_eq!(second["version_number"], 2);
        assert_eq!(second["is_default"], false);
        let second_id = second["id"].as_str().unwrap().to_owned();
        assert_eq!(can_list_groups().await, StatusCode::OK);

        let switched = request
            .post(&format!("/api/policies/{policy_id}/versions/{second_id}:set-default"))
            .add_header("Authorization", &auth)
            .await;
        switched.assert_status_ok();
        assert_eq!(switched.json::<Value>()["is_default"], true);
        assert_eq!(can_list_groups().await, StatusCode::FORBIDDEN);
        let current = request
            .get(&format!("/api/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .await
            .json::<Value>();
        assert_eq!(current["default_version_id"], second_id);
        assert_eq!(current["document"]["Statement"][0]["Effect"], "Deny");

        let versions = request
            .get(&format!("/api/policies/{policy_id}/versions"))
            .add_header("Authorization", &auth)
            .await
            .json::<Vec<Value>>();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions.iter().filter(|v| v["is_default"] == true).count(), 1);
        let version = request
            .get(&format!("/api/policies/{policy_id}/versions/{first_version}"))
            .add_header("Authorization", &auth)
            .await;
        version.assert_status_ok();
        assert_eq!(version.json::<Value>()["version_number"], 1);

        request
            .delete(&format!("/api/policies/{policy_id}/versions/{second_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .post(&format!("/api/policies/{policy_id}/versions/{first_version}:set-default"))
            .add_header("Authorization", &auth)
            .await
            .assert_status_ok();
        assert_eq!(can_list_groups().await, StatusCode::OK);
        request
            .delete(&format!("/api/policies/{policy_id}/versions/{second_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // At most five versions.
        for _ in 0..4 {
            request
                .post(&format!("/api/policies/{policy_id}/versions"))
                .add_header("Authorization", &auth)
                .json(&json!({"document":allow("iam:ListUsers")}))
                .await
                .assert_status(StatusCode::CREATED);
        }
        request
            .post(&format!("/api/policies/{policy_id}/versions"))
            .add_header("Authorization", &auth)
            .json(&json!({"document":allow("iam:ListUsers")}))
            .await
            .assert_status(StatusCode::CONFLICT);

        let policy_path = format!("/api/policies/{policy_id}");
        request
            .delete(&policy_path)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        assert_eq!(can_list_groups().await, StatusCode::OK);
        request
            .delete(&format!("/api/users/{user_id}/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .delete(&policy_path)
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get(&format!("/api/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn managed_policies_have_one_generated_version_and_cannot_be_changed() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {key}");
        let listed = request
            .get("/api/policies")
            .add_header("Authorization", &auth)
            .await;
        listed.assert_status_ok();
        let listed = listed.json::<Vec<Value>>();
        let names: Vec<&str> = listed.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, [
                "AdministratorAccess",
                "PowerUserAccess",
                "ReadOnlyAccess",
                "WorkspaceViewerAccess",
                "WorkspaceContributorAccess"
            ]);
        assert!(listed.iter().all(|p| p["managed"] == true));
        let administrator = &listed[0];
        assert_eq!(
            administrator["yrn"],
            "yrn:iam::policy/AdministratorAccess"
        );
        assert_eq!(
            administrator["document"],
            json!({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]})
        );
        let policy_id = administrator["id"].as_str().unwrap();

        let versions = request
            .get(&format!("/api/policies/{policy_id}/versions"))
            .add_header("Authorization", &auth)
            .await
            .json::<Vec<Value>>();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0]["id"], administrator["default_version_id"]);
        request
            .get(&format!(
                "/api/policies/{policy_id}/versions/{}",
                administrator["default_version_id"].as_str().unwrap()
            ))
            .add_header("Authorization", &auth)
            .await
            .assert_status_ok();
        request
            .post(&format!("/api/policies/{policy_id}/versions"))
            .add_header("Authorization", &auth)
            .json(&json!({"document":allow("iam:ListGroups")}))
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .post(&format!(
                "/api/policies/{policy_id}/versions/{}:set-default",
                administrator["default_version_id"].as_str().unwrap()
            ))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);
        request
            .delete(&format!("/api/policies/{policy_id}"))
            .add_header("Authorization", &auth)
            .await
            .assert_status(StatusCode::CONFLICT);

        // ReadOnlyAccess reads and lists but writes nothing.
        let reader = fixtures::create_iam_reader(&ctx, tenant.tenant_id).await;
        let reader_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, reader).await;
        let reader_auth = format!("Bearer {reader_key}");
        request
            .get("/api/policies")
            .add_header("Authorization", &reader_auth)
            .await
            .assert_status_ok();
        request
            .post("/api/groups")
            .add_header("Authorization", &reader_auth)
            .json(&json!({"name":"denied"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}
