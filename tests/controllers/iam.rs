use crate::requests::boot_request_with;
use axum::http::StatusCode;
use sea_orm::TransactionTrait;
use yorishiro::App;

#[tokio::test]
async fn unselected_lists_do_not_expose_workspaces_without_iam_grants() {
    use crate::requests::fixtures;
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
        let root = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let other_workspace = fixtures::rest_create_workspace(&request, &root).await;
        for workspace in [tenant.workspace_id, other_workspace] {
            request.post("/api/schemas")
                .add_header("Authorization", format!("Bearer {root}"))
                .add_header("x-workspace-id", workspace.to_string())
                .json(&serde_json::json!({"template_id":"general-notes"}))
                .await.assert_status(StatusCode::CREATED);
        }
        let response = request.get("/api/schemas")
            .add_header("Authorization", format!("Bearer {root}")).await;
        response.assert_status_ok();
        assert_eq!(response.json::<Vec<serde_json::Value>>().len(), 2);
        let mut entities = Vec::new();
        for title in ["private source", "private target"] {
            let response = request.post("/api/entities")
                .add_header("Authorization", format!("Bearer {root}"))
                .add_header("x-workspace-id", tenant.workspace_id.to_string())
                .json(&serde_json::json!({"schema_name":"general-notes","entity_type":"note","data":{"title":title}}))
                .await;
            response.assert_status(StatusCode::CREATED);
            entities.push(response.json::<serde_json::Value>()["entity_id"].clone());
        }
        request.post("/api/relations")
            .add_header("Authorization", format!("Bearer {root}"))
            .add_header("x-workspace-id", tenant.workspace_id.to_string())
            .json(&serde_json::json!({"source_id":entities[0],"target_id":entities[1],"relation_type":"references","properties":{}}))
            .await.assert_status(StatusCode::CREATED);
        for (path, count) in [("/api/entities", 2), ("/api/relations", 1)] {
            let response = request.get(path)
                .add_header("Authorization", format!("Bearer {root}")).await;
            response.assert_status_ok();
            assert_eq!(response.json::<Vec<serde_json::Value>>().len(), count);
        }
        let txn = ctx.db.begin().await.unwrap();
        let member = yorishiro::models::tenancy::register_member(
            &txn, tenant.tenant_id, &fixtures::unique_email("no-grants"), "hunter2-hunter2", None,
        ).await.unwrap();
        txn.commit().await.unwrap();
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, member.id).await;
        for path in ["/api/schemas", "/api/entities", "/api/relations", "/api/search?query_text=private"] {
            let response = request.get(path).add_header("Authorization", format!("Bearer {key}")).await;
            response.assert_status_ok();
            assert_eq!(response.json::<serde_json::Value>(), serde_json::json!([]));
        }
        let entity_path = format!("/api/entities/{}", entities[0].as_str().unwrap());
        request.get(&entity_path).add_header("Authorization", format!("Bearer {key}"))
            .await.assert_status(StatusCode::NOT_FOUND);
        request.get(&entity_path).add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-workspace-id", tenant.workspace_id.to_string())
            .await.assert_status(StatusCode::FORBIDDEN);
        let policy = request.post("/api/policies")
            .add_header("Authorization", format!("Bearer {root}"))
            .json(&serde_json::json!({"name":"one-workspace","document":{"Version":"2012-10-17","Statement":[{
                "Effect":"Allow","Action":["schema:ListSchemas"],"Resource":[format!("yrn:workspace:{}:{}", tenant.tenant_id, tenant.workspace_id)]
            }]}})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy = policy.json::<serde_json::Value>();
        request.put(&format!("/api/users/{}/policies/{}", member.id, policy["policy_id"].as_str().unwrap()))
            .add_header("Authorization", format!("Bearer {root}")).await.assert_status(StatusCode::NO_CONTENT);
        let response = request.get("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}")).await;
        response.assert_status_ok();
        let rows = response.json::<Vec<serde_json::Value>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["workspace_id"], tenant.workspace_id.to_string());
        request.post("/api/entities")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"schema_name":"general-notes","entity_type":"note","data":{"title":"denied"}}))
            .await.assert_status(StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
async fn entity_creation_selects_the_only_authorized_workspace() {
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request, &ctx, crate::requests::fixtures::TenantArgs::default(),
        ).await;
        request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&serde_json::json!({"template_id":"general-notes"}))
            .await.assert_status(StatusCode::CREATED);
        let administrator = crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        let input = serde_json::json!({"schema_name":"general-notes","entity_type":"note","data":{"title":"implicit"}});
        let created = request.post("/api/entities")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&input).await;
        created.assert_status(StatusCode::CREATED);
        assert_eq!(created.json::<serde_json::Value>()["workspace_id"], tenant.workspace_id.to_string());
        crate::requests::fixtures::rest_create_workspace(&request, &key).await;
        request.post("/api/entities")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&input).await.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    }).await;
}

#[tokio::test]
async fn relation_operations_resolve_the_source_and_relation_workspaces() {
    use crate::requests::fixtures;
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        request.post("/api/schemas").add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"template_id":"general-notes"})).await.assert_status(StatusCode::CREATED);
        let mut ids = Vec::new();
        for title in ["source", "target"] {
            let response = request.post("/api/entities").add_header("Authorization", format!("Bearer {key}"))
                .json(&serde_json::json!({"schema_name":"general-notes","entity_type":"note","data":{"title":title}})).await;
            response.assert_status(StatusCode::CREATED);
            ids.push(response.json::<serde_json::Value>()["entity_id"].clone());
        }
        fixtures::rest_create_workspace(&request, &key).await;
        let relation = request.post("/api/relations").add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"source_id":ids[0],"target_id":ids[1],"relation_type":"references","properties":{}})).await;
        relation.assert_status(StatusCode::CREATED);
        let id = relation.json::<serde_json::Value>()["relation_id"].as_str().unwrap().to_owned();
        let path = format!("/api/relations/{id}");
        request.get(&path).add_header("Authorization", format!("Bearer {key}")).await.assert_status_ok();
        request.patch(&path).add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"status":"archived"})).await.assert_status_ok();
        request.delete(&path).add_header("Authorization", format!("Bearer {key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
    }).await;
}

#[tokio::test]
async fn schema_ids_resolve_the_workspace_and_creation_refuses_ambiguity() {
    use crate::requests::fixtures;
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant =
            fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        let created = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"template_id":"general-notes"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let id = created.json::<serde_json::Value>()["schema"]["schema_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let txn = ctx.db.begin().await.unwrap();
        let member = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &fixtures::unique_email("schema-no-grant"),
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let denied = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, member.id).await;
        request
            .get(&format!("/api/schemas/{id}"))
            .add_header("Authorization", format!("Bearer {denied}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let other = fixtures::rest_create_workspace(&request, &key).await;
        request
            .get(&format!("/api/schemas/{id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status_ok();
        request
            .get(&format!("/api/schemas/{id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-workspace-id", other.to_string())
            .await
            .assert_status(StatusCode::NOT_FOUND);
        request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"template_id":"general-notes"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .post(&format!("/api/schemas/{id}:archive"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status_ok();
        request
            .delete(&format!("/api/schemas/{id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::CONFLICT);
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one account history compares root sessions with restricted and delegated credentials over REST and MCP"
)]
async fn root_sessions_have_full_access_but_issued_keys_retain_their_limits() {
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let key = crate::requests::fixtures::create_key(
            &ctx.db,
            tenant.workspace_id,
            Some(tenant.owner_id),
        )
        .await
        .unwrap()
        .plaintext;
        let root =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let named_login = request.post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {root}"))
            .json(&serde_json::json!({"name":"login"}))
            .await;
        named_login.assert_status(StatusCode::CREATED);
        let named_login = named_login.json::<serde_json::Value>();
        request.post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", named_login["access_key"].as_str().unwrap()))
            .json(&serde_json::json!({"name":"denied"}))
            .await.assert_status(StatusCode::FORBIDDEN);
        request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"name":"denied"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let response = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {root}"))
            .json(&serde_json::json!({"name":"root-created"}))
            .await;
        response.assert_status(StatusCode::CREATED);
        let workspace = response.json::<serde_json::Value>()["workspace_id"]
            .as_str()
            .unwrap()
            .to_owned();
        request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {root}"))
            .add_header("x-workspace-id", &workspace)
            .json(&serde_json::json!({"template_id":"general-notes"}))
            .await
            .assert_status(StatusCode::CREATED);
        request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {root}"))
            .add_header("x-workspace-id", &workspace)
            .await
            .assert_status_ok();
        let session = crate::controllers::mcp::initialize(&request).await;
        let result = crate::controllers::mcp::call_tool(
            &request,
            &session,
            &root,
            "list_schemas",
            serde_json::json!({"workspace_id":workspace}),
        )
        .await;
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert!(result.get("error").is_none(), "{result}");
        let result = crate::controllers::mcp::call_tool(
            &request,
            &session,
            &root,
            "list_schemas",
            serde_json::json!({}),
        )
        .await;
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert!(result.get("error").is_none(), "{result}");
        let administrator =
            crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let delegated =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator)
                .await;
        let result = crate::controllers::mcp::call_tool(
            &request,
            &session,
            &delegated,
            "list_schemas",
            serde_json::json!({}),
        )
        .await;
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert!(result.get("error").is_none(), "{result}");
        let response = request
            .get("/api/schemas")
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await;
        response.assert_status_ok();
        assert_eq!(response.json::<Vec<serde_json::Value>>().len(), 1);
        request.get("/api/relations")
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status_ok();
        request.post("/api/entities")
            .add_header("Authorization", format!("Bearer {root}"))
            .add_header("x-workspace-id", &workspace)
            .json(&serde_json::json!({"schema_name":"general-notes","entity_type":"note","data":{"title":"visible"}}))
            .await.assert_status(StatusCode::CREATED);
        let response = request.get("/api/entities")
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await;
        response.assert_status_ok();
        assert_eq!(response.json::<Vec<serde_json::Value>>().len(), 1);
        let id = response.json::<Vec<serde_json::Value>>()[0]["entity_id"].as_str().unwrap().to_owned();
        request.get(&format!("/api/entities/{id}"))
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status_ok();
        request.put(&format!("/api/entities/{id}"))
            .add_header("Authorization", format!("Bearer {delegated}"))
            .json(&serde_json::json!({"data":{"title":"updated"}}))
            .await.assert_status_ok();
        request.delete(&format!("/api/entities/{id}"))
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await.assert_status(StatusCode::NO_CONTENT);
    })
    .await;
}

#[tokio::test]
async fn iam_writes_are_noted_with_their_action_on_the_completed_request_row() {
    use crate::requests::fixtures;
    use sea_orm::EntityTrait;
    boot_request_with::<App, _, _>(
        &[crate::requests::max_tenants(1)],
        |request, ctx| async move {
            let tenant =
                fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
            let root = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
            let auth = format!("Bearer {root}");
            let group = request
                .post("/api/groups")
                .add_header("Authorization", &auth)
                .json(&serde_json::json!({"name": "operators"}))
                .await;
            group.assert_status(StatusCode::CREATED);
            let group_id = group.json::<serde_json::Value>()["group_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let policy_id = fixtures::create_policy(
                &request,
                &auth,
                "audited",
                &fixtures::policy_document("Allow", &["iam:ListGroups"], &["*".to_owned()]),
            )
            .await;
            request
                .patch(&format!("/api/groups/{group_id}"))
                .add_header("Authorization", &auth)
                .json(&serde_json::json!({"name":"renamed"}))
                .await
                .assert_status_ok();
            request
                .put(&format!("/api/groups/{group_id}/policies/{policy_id}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
            request
                .delete(&format!("/api/groups/{group_id}/policies/{policy_id}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
            request
                .delete(&format!("/api/groups/{group_id}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);

            let actions: std::collections::HashSet<String> =
                yorishiro::models::api_events::Entity::find()
                    .all(&ctx.db)
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|event| event.phase == "completed")
                    .flat_map(|event| event.resources.as_array().cloned().unwrap_or_default())
                    .filter_map(|resource| resource["action"].as_str().map(str::to_owned))
                    .collect();
            for expected in [
                "iam:CreateGroup",
                "iam:CreatePolicy",
                "iam:UpdateGroup",
                "iam:AttachGroupPolicy",
                "iam:DetachGroupPolicy",
                "iam:DeleteGroup",
            ] {
                assert!(actions.contains(expected), "{expected} in {actions:?}");
            }
        },
    )
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one user and its policies are simulated for every decision and compared with real requests"
)]
async fn simulation_reports_decisions_and_matching_policies_like_real_requests() {
    use crate::requests::fixtures;
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant =
            fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
        let root = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {root}");
        let user = request
            .post("/api/users")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"user_name":"erin","password":"tenant-user-password"}))
            .await
            .json::<serde_json::Value>();
        let user_id = user["user_id"].as_str().unwrap().to_owned();
        for (name, effect, action) in [
            ("lists", "Allow", "iam:List*"),
            ("no-user-listing", "Deny", "iam:ListUsers"),
            (
                "workspace-reader",
                "Allow",
                "workspace:GetWorkspace",
            ),
        ] {
            let resource = if action.starts_with("workspace:") {
                fixtures::workspace_yrn(tenant.tenant_id, tenant.workspace_id)
            } else {
                "*".to_owned()
            };
            let policy = fixtures::create_policy(
                &request,
                &auth,
                name,
                &fixtures::policy_document(effect, &[action], &[resource]),
            )
            .await;
            request
                .put(&format!("/api/users/{user_id}/policies/{policy}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
        }
        let simulate = |body: serde_json::Value| {
            request
                .post("/api/simulate-principal-policy")
                .add_header("Authorization", &auth)
                .json(&body)
        };
        macro_rules! decided {
            ($response:expr) => {{
                let response = $response;
                response.assert_status_ok();
                let body = response.json::<serde_json::Value>();
                let mut matched: Vec<String> = body["matched_policies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| name.as_str().unwrap().to_owned())
                    .collect();
                matched.sort();
                (body["decision"].as_str().unwrap().to_owned(), matched)
            }};
        }
        assert_eq!(
            decided!(simulate(serde_json::json!({"user_id":user_id,"action":"iam:ListGroups"})).await),
            ("allowed".to_owned(), vec!["lists".to_owned()])
        );
        assert_eq!(
            decided!(simulate(serde_json::json!({"user_id":user_id,"action":"iam:ListUsers"})).await),
            ("explicit_deny".to_owned(), vec!["no-user-listing".to_owned()])
        );
        assert_eq!(
            decided!(simulate(serde_json::json!({"user_id":user_id,"action":"iam:CreateGroup"})).await),
            ("implicit_deny".to_owned(), Vec::new())
        );
        let workspace = fixtures::workspace_yrn(tenant.tenant_id, tenant.workspace_id);
        assert_eq!(
            decided!(simulate(serde_json::json!({"user_id":user_id,"action":"workspace:GetWorkspace","resource":workspace})).await),
            ("allowed".to_owned(), vec!["workspace-reader".to_owned()])
        );
        assert_eq!(
            decided!(simulate(serde_json::json!({"user_id":user_id,"action":"workspace:GetWorkspace"})).await).0,
            "implicit_deny",
            "a statement for one workspace does not allow every workspace"
        );
        // The caller is the default user, and the tenant root may do everything.
        assert_eq!(
            decided!(simulate(serde_json::json!({"action":"iam:CreateGroup"})).await).0,
            "allowed"
        );
        simulate(serde_json::json!({"user_id":user_id,"action":"nonsense"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        simulate(serde_json::json!({"user_id":user_id,"action":"workspace:GetWorkspace","resource":fixtures::tenant_yrn(tenant.tenant_id)}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        simulate(serde_json::json!({"user_id":uuid::Uuid::new_v4(),"action":"iam:ListGroups"}))
            .await
            .assert_status(StatusCode::NOT_FOUND);

        // The simulation agrees with the real request.
        let user_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, user_id.parse().unwrap()).await;
        let denied = request
            .get("/api/users")
            .add_header("Authorization", format!("Bearer {user_key}"))
            .await;
        denied.assert_status(StatusCode::FORBIDDEN);
        let error = &denied.json::<serde_json::Value>()["error"];
        assert_eq!(error["code"], "access_denied");
        assert_eq!(
            error["message"],
            format!(
                "IAM policy explicitly denies iam:ListUsers on yrn:tenant:{}: no-user-listing",
                tenant.tenant_id
            )
        );
        request
            .get("/api/groups")
            .add_header("Authorization", format!("Bearer {user_key}"))
            .await
            .assert_status_ok();
        // Simulating needs its own permission.
        request
            .post("/api/simulate-principal-policy")
            .add_header("Authorization", format!("Bearer {user_key}"))
            .json(&serde_json::json!({"action":"iam:ListGroups"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the same delegate and target exercise direct grants, group grants and group membership"
)]
async fn only_administrators_can_grant_administrator_access() {
    use crate::requests::fixtures;
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant =
            fixtures::TestTenant::create(&request, &ctx, fixtures::TenantArgs::default()).await;
        let root = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let root_auth = format!("Bearer {root}");
        let administrator_policy =
            yorishiro::models::iam::builtin::managed_policy_id("AdministratorAccess").unwrap();
        let mut users = Vec::new();
        for name in ["delegate", "target", "other"] {
            let created = request
                .post("/api/users")
                .add_header("Authorization", &root_auth)
                .json(&serde_json::json!({"user_name":name,"password":"tenant-user-password"}))
                .await;
            created.assert_status(StatusCode::CREATED);
            users.push(
                created.json::<serde_json::Value>()["user_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        // The delegate holds every IAM action except AdministratorAccess itself.
        let iam_policy = fixtures::create_policy(
            &request,
            &root_auth,
            "iam-manager",
            &fixtures::policy_document("Allow", &["iam:*"], &["*".to_owned()]),
        )
        .await;
        request
            .put(&format!("/api/users/{}/policies/{iam_policy}", users[0]))
            .add_header("Authorization", &root_auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        let delegate_key =
            fixtures::issue_tenant_key(&ctx, tenant.tenant_id, users[0].parse().unwrap()).await;
        let delegate = format!("Bearer {delegate_key}");

        let group = request
            .post("/api/groups")
            .add_header("Authorization", &delegate)
            .json(&serde_json::json!({"name":"admins"}))
            .await;
        group.assert_status(StatusCode::CREATED);
        let group_id = group.json::<serde_json::Value>()["group_id"]
            .as_str()
            .unwrap()
            .to_owned();
        // A group without AdministratorAccess can be joined and given other policies.
        request
            .put(&format!("/api/groups/{group_id}/members/{}", users[1]))
            .add_header("Authorization", &delegate)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .delete(&format!("/api/groups/{group_id}/members/{}", users[1]))
            .add_header("Authorization", &delegate)
            .await
            .assert_status(StatusCode::NO_CONTENT);

        let grant_to_user = format!("/api/users/{}/policies/{administrator_policy}", users[1]);
        let grant_to_group = format!("/api/groups/{group_id}/policies/{administrator_policy}");
        for path in [&grant_to_user, &grant_to_group] {
            let denied = request
                .put(path)
                .add_header("Authorization", &delegate)
                .await;
            denied.assert_status(StatusCode::FORBIDDEN);
        }
        // The tenant root can, and the group then guards its own membership.
        request
            .put(&grant_to_group)
            .add_header("Authorization", &root_auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .put(&format!("/api/groups/{group_id}/members/{}", users[1]))
            .add_header("Authorization", &delegate)
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .put(&format!("/api/groups/{group_id}/members/{}", users[1]))
            .add_header("Authorization", &root_auth)
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // A user who holds AdministratorAccess through a group can grant it on.
        let member_key =
            fixtures::issue_tenant_key(&ctx, tenant.tenant_id, users[1].parse().unwrap()).await;
        request
            .put(&format!(
                "/api/users/{}/policies/{administrator_policy}",
                users[2]
            ))
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        // Detaching is not a grant.
        request
            .delete(&format!(
                "/api/users/{}/policies/{administrator_policy}",
                users[2]
            ))
            .add_header("Authorization", &delegate)
            .await
            .assert_status(StatusCode::NO_CONTENT);
    })
    .await;
}
