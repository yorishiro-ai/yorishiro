use crate::requests::boot_request_with;
use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, TransactionTrait};
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
            entities.push(response.json::<serde_json::Value>()["id"].clone());
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
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {root}"))
            .json(&serde_json::json!({"name":"one-workspace","document":{"statements":[{
                "effect":"allow","actions":["schema_list"],"resources":[{
                    "type":"workspace","tenant_id":tenant.tenant_id,"workspace_id":tenant.workspace_id
                }]
            }]}})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy = policy.json::<serde_json::Value>();
        request.put(&format!("/api/iam/users/{}/policies/{}", member.id, policy["id"].as_str().unwrap()))
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
            ids.push(response.json::<serde_json::Value>()["id"].clone());
        }
        fixtures::rest_create_workspace(&request, &key).await;
        let relation = request.post("/api/relations").add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"source_id":ids[0],"target_id":ids[1],"relation_type":"references","properties":{}})).await;
        relation.assert_status(StatusCode::CREATED);
        let id = relation.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
        let path = format!("/api/relations/{id}");
        request.get(&path).add_header("Authorization", format!("Bearer {key}")).await.assert_status_ok();
        request.put(&format!("{path}/status")).add_header("Authorization", format!("Bearer {key}"))
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
        let id = created.json::<serde_json::Value>()["schema"]["id"]
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
            .assert_status(StatusCode::NOT_FOUND);
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
            .post(&format!("/api/schemas/{id}/archive"))
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
    use yorishiro::models::api_keys::{ApiKeyScope, Entity};
    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let key = Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap()
        .plaintext;
        let root =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let named_login = request.post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {root}"))
            .json(&serde_json::json!({"name":"login","tenant_scoped":true,"scope":"read"}))
            .await;
        named_login.assert_status(StatusCode::CREATED);
        let named_login = named_login.json::<serde_json::Value>();
        request.post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", named_login["full_key"].as_str().unwrap()))
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
        let workspace = response.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        request
            .get("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-workspace-id", &workspace)
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"template_id":"general-notes"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
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
        let id = response.json::<Vec<serde_json::Value>>()[0]["id"].as_str().unwrap().to_owned();
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
#[expect(
    clippy::too_many_lines,
    reason = "the same credentials and audit history are needed to compare denied, rolled-back, and committed IAM mutations across REST and MCP"
)]
async fn iam_management_preserves_credential_boundaries_and_audit_atomicity() {
    let overrides = [crate::requests::max_tenants(1)];
    boot_request_with::<App, _, _>(&overrides, |request, ctx| async move {
            let response = request
                .post("/setup")
                .json(&serde_json::json!({
                    "email": "owner@example.com",
                    "password": "hunter2-hunter2",
                }))
                .await;
            assert_eq!(response.status_code(), 201, "{}", response.text());
            let body: serde_json::Value = response.json();
            let api_key = body["api_key"].as_str().unwrap().to_string();
            let workspace_id = crate::requests::fixtures::rest_create_workspace(&request, &api_key).await;

            let group = request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name": "operators"}))
                .await;
            assert_eq!(group.status_code(), StatusCode::CREATED, "{}", group.text());
            for (name, status) in [
                ("operators", StatusCode::CONFLICT),
                (" ", StatusCode::UNPROCESSABLE_ENTITY),
            ] {
                request
                    .post("/api/iam/groups")
                    .add_header("Authorization", format!("Bearer {api_key}"))
                    .json(&serde_json::json!({"name":name}))
                    .await
                    .assert_status(status);
            }
            let denied = request
                .post("/api/iam/groups")
                .json(&serde_json::json!({"name": "unauthorized"}))
                .await;
            denied.assert_status(StatusCode::UNAUTHORIZED);
            let events = yorishiro::models::iam_audit_events::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].action, "group_create");
            let tenant_id = body["tenant_id"].as_str().unwrap().parse().unwrap();
            let user_id = crate::requests::fixtures::create_iam_administrator(&ctx, tenant_id).await;
            let audit_key = yorishiro::models::api_keys::Entity::create_tenant_api_key(
                &ctx.db,
                tenant_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user_id),
                true,
                "iam-audit",
            )
            .await
            .unwrap();
            let no_audit_key = yorishiro::models::api_keys::Entity::create_tenant_api_key(
                &ctx.db,
                tenant_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user_id),
                false,
                "without-audit",
            )
            .await
            .unwrap();
            request
                .get("/api/iam/audit")
                .add_header(
                    "Authorization",
                    format!("Bearer {}", no_audit_key.plaintext),
                )
                .await
                .assert_status(StatusCode::FORBIDDEN);
            let audit_rest = request
                .get("/api/iam/audit?limit=1")
                .add_header("Authorization", format!("Bearer {}", audit_key.plaintext))
                .await;
            audit_rest.assert_status_ok();
            assert_eq!(
                audit_rest.json::<serde_json::Value>()[0]["action"],
                "group_create"
            );

            let session = crate::controllers::mcp::initialize(&request).await;
            let audit_mcp = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &audit_key.plaintext,
                "list_iam_audit",
                serde_json::json!({"limit":1}),
            )
            .await;
            let audit_mcp: serde_json::Value =
                serde_json::from_str(audit_mcp["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(audit_mcp, audit_rest.json::<serde_json::Value>());
            let policy = serde_json::json!({"name":"operators", "document":{"statements":[{
                "effect":"allow", "actions":["group_create", "group_list", "group_get", "policy_list", "policy_get", "group_member_list", "user_policy_list", "group_policy_list", "template_library_create", "template_library_update", "template_library_delete", "template_library_fork", "workspace_create", "workspace_list"],
                "resources":[{"type":"tenant","tenant_id":body["tenant_id"]}]
            }, {
                "effect":"allow", "actions":["workspace_update", "workspace_get"],
                "resources":[{"type":"workspace","tenant_id":body["tenant_id"],"workspace_id":workspace_id}]
            }, {
                "effect":"allow", "actions":["workspace_delete"],
                "resources":[{"type":"all_workspaces","tenant_id":body["tenant_id"]}]
            }]}});
            let created =
                crate::controllers::mcp::call_tool(&request, &session, &api_key, "create_iam_policy", policy).await;
            assert_ne!(created["result"]["isError"], true, "{created}");
            let policy_result: serde_json::Value =
                serde_json::from_str(created["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            let events = yorishiro::models::iam_audit_events::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(events.len(), 2);
            assert!(events.iter().any(|event| event.action == "policy_create"));

            // Direct write (c): deterministic fault injection making the audit insert fail.
            crate::requests::fixtures::rename_table(&ctx.db, "iam_audit_events", "unavailable_iam_audit_events").await;
            let failed_rest = request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name":"rest-rollback"}))
                .await;
            let failed_mcp = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &api_key,
                "create_iam_group",
                serde_json::json!({"name":"mcp-rollback"}),
            )
            .await;
            crate::requests::fixtures::rename_table(&ctx.db, "unavailable_iam_audit_events", "iam_audit_events").await;
            failed_rest.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(failed_mcp["result"]["isError"], true, "{failed_mcp}");
            let groups = yorishiro::models::iam_groups::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .filter(|group| !group.name.starts_with("Tenant "))
                .collect::<Vec<_>>();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].name, "operators");

            let no_workspace = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            no_workspace.assert_status(StatusCode::OK);
            assert!(no_workspace.json::<serde_json::Value>()["workspace_id"].is_null());
            request
                .get("/api/tenants/current")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::OK);
            request
                .get("/api/schemas")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::OK);
            let whoami = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", workspace_id.to_string())
                .await;
            assert_eq!(whoami.status_code(), StatusCode::OK, "{}", whoami.text());
            let identity: serde_json::Value = whoami.json();
            assert_eq!(identity["workspace_id"], workspace_id.to_string());

            let created = request
                .post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", workspace_id.to_string())
                .json(&serde_json::json!({ "name": "Second" }))
                .await;
            assert_eq!(created.status_code(), 201, "{}", created.text());
            let second_id: uuid::Uuid = created.json::<serde_json::Value>()["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();

            let ambiguous = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await;
            assert_eq!(ambiguous.status_code(), 200, "{}", ambiguous.text());
            assert!(ambiguous.json::<serde_json::Value>()["workspace_id"].is_null());

            {
                let whoami = request
                    .get("/api/whoami")
                    .add_header("Authorization", format!("Bearer {api_key}"))
                    .add_header("x-workspace-id", second_id.to_string())
                    .await;
                assert_eq!(whoami.status_code(), 200, "{}", whoami.text());
                let chosen: serde_json::Value = whoami.json();
                assert_eq!(chosen["workspace_id"], second_id.to_string());
            }

            let user_id = crate::requests::fixtures::create_iam_administrator(&ctx, tenant_id).await;
            let scoped_key = yorishiro::models::api_keys::Entity::create_api_key(
                &ctx.db,
                workspace_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user_id),
                false,
            )
            .await
            .unwrap();
            let denied = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &scoped_key.plaintext,
                "create_iam_group",
                serde_json::json!({"name":"binding-denied"}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&serde_json::json!({"name":"binding-denied"}))
                .await
                .assert_status(StatusCode::FORBIDDEN);
            assert_eq!(
                yorishiro::models::iam_audit_events::Entity::find()
                    .all(&ctx.db)
                    .await
                    .unwrap()
                    .len(),
                2
            );
            request
                .get("/api/tenants/current")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await
                .assert_status(StatusCode::FORBIDDEN);
            let scoped = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .add_header("x-workspace-id", second_id.to_string())
                .await;
            assert_eq!(scoped.status_code(), 403, "{}", scoped.text());
            let scoped = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await;
            assert_eq!(scoped.status_code(), 200, "{}", scoped.text());
            let identity: serde_json::Value = scoped.json();
            assert_eq!(identity["workspace_id"], workspace_id.to_string());
            let group_id = group.json::<serde_json::Value>()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            request
                .put(&format!("/api/iam/groups/{group_id}"))
                .add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name":"renamed"}))
                .await
                .assert_status_ok();
            let deleted = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &api_key,
                "delete_iam_group",
                serde_json::json!({"group_id":group_id}),
            )
            .await;
            assert_ne!(deleted["result"]["isError"], true, "{deleted}");
            let events = yorishiro::models::iam_audit_events::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(events.len(), 4);
            assert!(events.iter().any(|event| event.action == "group_update"));
            assert!(events.iter().any(|event| event.action == "group_delete"));
            let user = yorishiro::models::user_users::create_user(
                &ctx.db,
                "iam-user@example.invalid",
                "test-password-123",
                None,
            )
            .await
            .unwrap();
            yorishiro::models::tenant_memberships::add_member(
                &ctx.db,
                tenant_id,
                user.id)
            .await
            .unwrap();
            let user_key = yorishiro::models::api_keys::Entity::create_tenant_api_key(
                &ctx.db,
                tenant_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user.id),
                false,
                "iam-user",
            )
            .await
            .unwrap();
            request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"implicit-denied"}))
                .await
                .assert_status(StatusCode::FORBIDDEN);
            let policy_id = policy_result["id"].as_str().unwrap();
            request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            request.get(&format!("/api/workspaces/{workspace_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NOT_FOUND);
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"implicit-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let template = serde_json::json!({"name":"iam-template", "definition":{"name":"iam-template",
                "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}
            }});
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&template).await.assert_status(StatusCode::FORBIDDEN);
            request.get("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let denied = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "list_iam_policies", serde_json::json!({})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            let attachment = format!("/api/iam/users/{}/policies/{policy_id}", user.id);
            request
                .put(&attachment)
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::NO_CONTENT);
            let allowed = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &user_key.plaintext,
                "create_iam_group",
                serde_json::json!({"name":"policy-allowed"}),
            )
            .await;
            assert_ne!(allowed["result"]["isError"], true, "{allowed}");
            let workspace_path = format!("/api/workspaces/{workspace_id}");
            let inventory = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
            inventory.assert_status_ok();
            let inventory = inventory.json::<serde_json::Value>();
            assert_eq!(inventory.as_array().unwrap().len(), 1);
            assert_eq!(inventory[0]["id"], workspace_id.to_string());
            let mcp_inventory = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            let mcp_inventory: serde_json::Value = serde_json::from_str(mcp_inventory["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(inventory, mcp_inventory);
            let discovered = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "discover_workspaces", serde_json::json!({})).await;
            let discovered: serde_json::Value = serde_json::from_str(discovered["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(discovered.as_array().unwrap().len(), 1);
            assert_eq!(discovered[0]["workspace_id"], workspace_id.to_string());
            request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_list = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            assert_eq!(bound_list["result"]["isError"], true, "{bound_list}");
            let detail = request.get(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
            detail.assert_status_ok();
            let mcp_detail = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":workspace_id})).await;
            let mcp_detail: serde_json::Value = serde_json::from_str(mcp_detail["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(detail.json::<serde_json::Value>(), mcp_detail);
            request.get(&format!("/api/workspaces/{second_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_detail = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":second_id})).await;
            assert_eq!(bound_detail["result"]["isError"], true, "{bound_detail}");
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"IAM workspace"})).await.assert_status_ok();
            let updated_workspace = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":workspace_id,"name":"MCP IAM workspace"})).await;
            assert_ne!(updated_workspace["result"]["isError"], true, "{updated_workspace}");
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"must-not-save","replace_max_entities":true,"max_entities":42}))
                .await.assert_status(StatusCode::FORBIDDEN);
            request.put(&format!("/api/workspaces/{second_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&serde_json::json!({"name":"binding-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let bound = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":second_id,"name":"binding-denied"})).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&template).await.assert_status(StatusCode::CREATED);
            let mut mcp_template = template.clone();
            mcp_template["name"] = serde_json::json!("iam-mcp-template");
            let created_template = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "create_template_library_item", mcp_template.clone()).await;
            assert_ne!(created_template["result"]["isError"], true, "{created_template}");
            let created_template: serde_json::Value = serde_json::from_str(
                created_template["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let template_id = created_template["id"].as_str().unwrap();
            request.put(&format!("/api/template-library/{template_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"description":"IAM update"})).await.assert_status_ok();
            let updated = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "update_template_library_item", serde_json::json!({"id":template_id,"description":"MCP IAM update"})).await;
            assert_ne!(updated["result"]["isError"], true, "{updated}");
            let forked = request.post(&format!("/api/template-library/{template_id}/fork"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"iam-rest-fork"})).await;
            forked.assert_status(StatusCode::CREATED);
            let fork_id = forked.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
            request.delete(&format!("/api/template-library/{fork_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NO_CONTENT);
            let forked = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "fork_template_library_item", serde_json::json!({"id":template_id,"name":"iam-mcp-fork"})).await;
            assert_ne!(forked["result"]["isError"], true, "{forked}");
            let forked: serde_json::Value = serde_json::from_str(forked["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let deleted = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "delete_template_library_item", serde_json::json!({"id":forked["id"]})).await;
            assert_ne!(deleted["result"]["isError"], true, "{deleted}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&template).await.assert_status(StatusCode::FORBIDDEN);
            let bound = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "create_template_library_item", mcp_template.clone()).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            let detached = crate::controllers::mcp::call_tool(
                &request,
                &session,
                &api_key,
                "detach_user_policy",
                serde_json::json!({"user_id":user.id,"policy_id":policy_id}),
            )
            .await;
            assert_ne!(detached["result"]["isError"], true, "{detached}");
            request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"revoked-denied"}))
                .await
                .assert_status(StatusCode::FORBIDDEN);
            let allowed_group: serde_json::Value = serde_json::from_str(allowed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let group_id = allowed_group["id"].as_str().unwrap();
            let member_path = format!("/api/iam/groups/{group_id}/members/{}", user.id);
            request.put(&member_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let attached = crate::controllers::mcp::call_tool(&request, &session, &api_key, "attach_group_policy",
                serde_json::json!({"group_id":group_id,"policy_id":policy_id})).await;
            assert_ne!(attached["result"]["isError"], true, "{attached}");
            for (path, tool, args) in [
                ("/api/iam/groups?limit=1".to_owned(), "list_iam_groups", serde_json::json!({"limit":1})),
                ("/api/iam/policies?limit=1".to_owned(), "list_iam_policies", serde_json::json!({"limit":1})),
                (format!("/api/iam/groups/{group_id}"), "get_iam_group", serde_json::json!({"group_id":group_id})),
                (format!("/api/iam/policies/{policy_id}"), "get_iam_policy", serde_json::json!({"policy_id":policy_id})),
                (format!("/api/iam/groups/{group_id}/members"), "list_iam_group_members", serde_json::json!({"group_id":group_id})),
                (format!("/api/iam/groups/{group_id}/policies"), "list_iam_group_policies", serde_json::json!({"group_id":group_id})),
                (format!("/api/iam/users/{}/policies", user.id), "list_iam_user_policies", serde_json::json!({"user_id":user.id})),
            ] {
                let rest = request.get(&path)
                    .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
                rest.assert_status_ok();
                let mcp = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext, tool, args).await;
                assert_ne!(mcp["result"]["isError"], true, "{mcp}");
                let mcp: serde_json::Value = serde_json::from_str(mcp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                assert_eq!(rest.json::<serde_json::Value>(), mcp, "{tool}");
            }
            let first = request.get("/api/iam/groups?limit=1")
                .add_header("Authorization", format!("Bearer {api_key}")).await.json::<serde_json::Value>();
            let all = request.get("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}")).await.json::<Vec<serde_json::Value>>();
            let next = request.get(&format!("/api/iam/groups?limit=1&offset={}", all.len()))
                .add_header("Authorization", format!("Bearer {api_key}")).await.json::<serde_json::Value>();
            assert_eq!(first.as_array().unwrap().len(), 1);
            assert_eq!(first[0], all[0]);
            assert_eq!(next, serde_json::json!([]));
            let txn = ctx.db.begin().await.unwrap();
            // Direct write (a): `create_tenant` enforces the one-tenant cap, and the IAM group operations are crate-private.
            let foreign_tenant = yorishiro::models::tenant_tenants::ActiveModel {
                name: Set("foreign".into()), ..Default::default()
            }.insert(&txn).await.unwrap();
            let foreign_group = yorishiro::models::iam_groups::ActiveModel {
                tenant_id: Set(foreign_tenant.id), name: Set("foreign".into()), ..Default::default()
            }.insert(&txn).await.unwrap();
            let foreign_workspace = yorishiro::models::tenancy::create_workspace(
                &txn, foreign_tenant.id, "foreign", None, None,
            ).await.unwrap();
            txn.commit().await.unwrap();
            request.get(&format!("/api/workspaces/{}", foreign_workspace.id))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NOT_FOUND);
            let foreign_detail = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":foreign_workspace.id})).await;
            assert_eq!(foreign_detail["result"]["isError"], true, "{foreign_detail}");
            let inventory = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.json::<serde_json::Value>();
            assert!(inventory.as_array().unwrap().iter().all(|row| row["tenant_id"] == body["tenant_id"]));
            request.get(&format!("/api/iam/groups/{}", foreign_group.id))
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NOT_FOUND);
            let foreign = crate::controllers::mcp::call_tool(&request, &session, &api_key, "list_iam_group_members",
                serde_json::json!({"group_id":foreign_group.id})).await;
            assert_eq!(foreign["result"]["isError"], true, "{foreign}");
            let groups = request.get("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.json::<serde_json::Value>();
            let mut names = groups
                .as_array()
                .unwrap()
                .iter()
                .map(|group| group["name"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            names.sort();
            assert_eq!(
                names,
                ["Tenant Administrator", "Tenant Reader", "Tenant Writer", "policy-allowed"],
                "only this tenant's groups are listed; the foreign group stays hidden"
            );
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"IAM created"})).await.assert_status(StatusCode::CREATED);
            let created_workspace = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "create_workspace", serde_json::json!({"name":"MCP IAM created"})).await;
            assert_ne!(created_workspace["result"]["isError"], true, "{created_workspace}");
            let created_workspace: serde_json::Value = serde_json::from_str(created_workspace["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let delete_id = created_workspace["id"].as_str().unwrap();
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_delete = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":delete_id})).await;
            assert_eq!(bound_delete["result"]["isError"], true, "{bound_delete}");
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&serde_json::json!({"name":"bound-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let bound = crate::controllers::mcp::call_tool(&request, &session, &scoped_key.plaintext,
                "create_workspace", serde_json::json!({"name":"bound-denied"})).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"group-allowed"})).await.assert_status(StatusCode::CREATED);
            request.delete(&member_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"removed-from-group"})).await.assert_status(StatusCode::FORBIDDEN);
            request.put(&member_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let deny = request.post("/api/iam/policies").add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name":"deny-group-create","document":{"statements":[{
                    "effect":"deny","actions":["group_create", "template_library_create", "template_library_update", "template_library_delete", "template_library_fork", "workspace_create"],"resources":[{"type":"tenant","tenant_id":tenant_id}]
                }, {
                    "effect":"deny","actions":["workspace_update", "workspace_get"],
                    "resources":[{"type":"workspace","tenant_id":tenant_id,"workspace_id":workspace_id}]
                }, {
                    "effect":"deny","actions":["workspace_delete"],
                    "resources":[{"type":"all_workspaces","tenant_id":tenant_id}]
                }]}})).await;
            deny.assert_status(StatusCode::CREATED);
            let deny_id = deny.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
            let deny_path = format!("/api/iam/users/{}/policies/{deny_id}", user.id);
            request.put(&deny_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let denied = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext, "create_iam_group",
                serde_json::json!({"name":"explicit-denied"})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"explicit-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let denied_workspace = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":workspace_id,"name":"explicit-denied"})).await;
            assert_eq!(denied_workspace["result"]["isError"], true, "{denied_workspace}");
            request.get(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NOT_FOUND);
            let denied_detail = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":workspace_id})).await;
            assert_eq!(denied_detail["result"]["isError"], true, "{denied_detail}");
            let inventory = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
            inventory.assert_status_ok();
            assert_eq!(inventory.json::<Vec<serde_json::Value>>(), [] as [serde_json::Value; 0]);
            let denied_list = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            let denied_list: serde_json::Value = serde_json::from_str(denied_list["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(denied_list, serde_json::json!([]));
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"explicit-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let denied_create = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "create_workspace", serde_json::json!({"name":"explicit-denied"})).await;
            assert_eq!(denied_create["result"]["isError"], true, "{denied_create}");
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let denied_delete = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":delete_id})).await;
            assert_eq!(denied_delete["result"]["isError"], true, "{denied_delete}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&template).await.assert_status(StatusCode::FORBIDDEN);
            let denied_template = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "create_template_library_item", mcp_template).await;
            assert_eq!(denied_template["result"]["isError"], true, "{denied_template}");
            for key in [&user_key.plaintext, &scoped_key.plaintext] {
                request.put(&format!("/api/template-library/{template_id}"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .json(&serde_json::json!({"description":"must-not-save"})).await.assert_status(StatusCode::FORBIDDEN);
                request.post(&format!("/api/template-library/{template_id}/fork"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .json(&serde_json::json!({"name":"must-not-fork"})).await.assert_status(StatusCode::FORBIDDEN);
                request.delete(&format!("/api/template-library/{template_id}"))
                    .add_header("Authorization", format!("Bearer {key}"))
                    .await.assert_status(StatusCode::FORBIDDEN);
                for (tool, args) in [
                    ("update_template_library_item", serde_json::json!({"id":template_id,"description":"must-not-save"})),
                    ("fork_template_library_item", serde_json::json!({"id":template_id,"name":"must-not-fork"})),
                    ("delete_template_library_item", serde_json::json!({"id":template_id})),
                ] {
                    let denied = crate::controllers::mcp::call_tool(&request, &session, key, tool, args).await;
                    assert_eq!(denied["result"]["isError"], true, "{denied}");
                }
            }
            request.delete(&deny_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NO_CONTENT);
            let deleted_workspace = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":second_id})).await;
            assert_ne!(deleted_workspace["result"]["isError"], true, "{deleted_workspace}");
            let workspaces = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.json::<serde_json::Value>();
            for workspace in workspaces.as_array().unwrap() {
                if workspace["id"] != workspace_id.to_string() {
                    request.delete(&format!("/api/workspaces/{}", workspace["id"].as_str().unwrap()))
                        .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                        .await.assert_status(StatusCode::NO_CONTENT);
                }
            }
            request.delete(&format!("/api/workspaces/{workspace_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::CONFLICT);
            let last = crate::controllers::mcp::call_tool(&request, &session, &user_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":workspace_id})).await;
            assert_eq!(last["result"]["isError"], true, "{last}");
            request.delete(&member_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"membership-revoked"})).await.assert_status(StatusCode::FORBIDDEN);
            request.put(&attachment).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let mut updated_policy = policy_result.clone();
            updated_policy["document"]["statements"][0]["effect"] = serde_json::json!("deny");
            request.put(&format!("/api/iam/policies/{policy_id}"))
                .add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name":"operators","document":updated_policy["document"]}))
                .await.assert_status_ok();
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"updated-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let restored = crate::controllers::mcp::call_tool(&request, &session, &api_key, "update_iam_policy",
                serde_json::json!({"policy_id":policy_id,"name":"operators","document":policy_result["document"]})).await;
            assert_ne!(restored["result"]["isError"], true, "{restored}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"updated-allowed"})).await.assert_status(StatusCode::CREATED);
            let deleted = crate::controllers::mcp::call_tool(&request, &session, &api_key, "delete_iam_policy",
                serde_json::json!({"policy_id":policy_id})).await;
            assert_ne!(deleted["result"]["isError"], true, "{deleted}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"deleted-denied"})).await.assert_status(StatusCode::FORBIDDEN);
        })
        .await;
}

#[tokio::test]
async fn assigned_built_in_groups_change_user_permissions_without_changing_identity() {
    use crate::requests::fixtures::{TenantArgs, TestTenant};
    use serde_json::{Value, json};

    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {root}");
        let user = request.post("/api/members/users").add_header("Authorization", &auth)
            .json(&json!({"user_name":"group-user","password":"group-user-password"})).await;
        user.assert_status(StatusCode::CREATED);
        let user_id = user.json::<Value>()["user_id"].as_str().unwrap().to_owned();
        let login = request.post("/auth/tenant/login")
            .json(&json!({"tenant_id":tenant.tenant_id,"user_name":"group-user","password":"group-user-password"})).await;
        login.assert_status_ok();
        let login = login.json::<Value>();
        let user_auth = format!("Bearer {}", login["api_key"].as_str().unwrap());
        request.get("/api/iam/policies").add_header("Authorization", &user_auth)
            .await.assert_status(StatusCode::FORBIDDEN);
        let groups = request.get("/api/iam/groups").add_header("Authorization", &auth).await;
        groups.assert_status_ok();
        let groups = groups.json::<Vec<Value>>();
        assert_eq!(groups.len(), 3);
        let session = crate::controllers::mcp::initialize(&request).await;
        let listed = crate::controllers::mcp::call_tool(&request, &session, &root, "list_iam_groups", json!({})).await;
        assert_ne!(listed["result"]["isError"], true, "{listed}");
        let listed: Value = serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(listed, json!(groups));
        let group = groups.iter().find(|r| r["name"] == "Tenant Administrator").unwrap();
        request.put(&format!("/api/iam/groups/{}", group["id"].as_str().unwrap()))
            .add_header("Authorization", &auth).json(&json!({"name":"changed"}))
            .await.assert_status(StatusCode::CONFLICT);
        request.delete(&format!("/api/iam/groups/{}", group["id"].as_str().unwrap()))
            .add_header("Authorization", &auth).await.assert_status(StatusCode::CONFLICT);
        let path = format!("/api/iam/groups/{}/members/{user_id}", group["id"].as_str().unwrap());
        request.put(&path).add_header("Authorization", &auth).await.assert_status(StatusCode::NO_CONTENT);
        request.get("/api/iam/policies").add_header("Authorization", &user_auth).await.assert_status_ok();
        let identity = request.get("/api/whoami").add_header("Authorization", &user_auth).await;
        identity.assert_status_ok();
        assert_eq!(identity.json::<Value>()["user_id"], user_id);
        request.put(&format!("/api/iam/groups/{}/members/{}", group["id"].as_str().unwrap(), uuid::Uuid::new_v4()))
            .add_header("Authorization", &auth).await.assert_status(StatusCode::NOT_FOUND);
        request.delete(&path).add_header("Authorization", &auth).await.assert_status(StatusCode::NO_CONTENT);
        request.get("/api/iam/policies").add_header("Authorization", &user_auth)
            .await.assert_status(StatusCode::FORBIDDEN);
        let custom = request.post("/api/iam/groups").add_header("Authorization", &auth)
            .json(&json!({"name":"custom-group"})).await;
        custom.assert_status(StatusCode::CREATED);
        let custom = custom.json::<Value>();
        let policies = request.get("/api/iam/policies").add_header("Authorization", &auth).await.json::<Vec<Value>>();
        let reader = policies.iter().find(|p| p["name"] == "Tenant Reader").unwrap();
        let attachment = format!("/api/iam/groups/{}/policies/{}", custom["id"].as_str().unwrap(), reader["id"].as_str().unwrap());
        request.put(&attachment).add_header("Authorization", &auth).await.assert_status(StatusCode::NO_CONTENT);
        request.put(&format!("/api/iam/groups/{}/policies/{}", group["id"].as_str().unwrap(), reader["id"].as_str().unwrap()))
            .add_header("Authorization", &auth).await.assert_status(StatusCode::CONFLICT);
        request.delete(&attachment).add_header("Authorization", &auth).await.assert_status(StatusCode::NO_CONTENT);
        // Direct write (a): `create_tenant` enforces the one-tenant cap, and the IAM group operations are crate-private.
        let txn = ctx.db.begin().await.unwrap();
        let foreign = yorishiro::models::tenant_tenants::ActiveModel {
            name: Set("foreign-group-tenant".into()), ..Default::default()
        }.insert(&txn).await.unwrap();
        let foreign_group = yorishiro::models::iam_groups::ActiveModel {
            tenant_id: Set(foreign.id), name: Set("foreign-group".into()), ..Default::default()
        }.insert(&txn).await.unwrap();
        txn.commit().await.unwrap();
        request.put(&format!("/api/iam/groups/{}/members/{user_id}", foreign_group.id))
            .add_header("Authorization", &auth).await.assert_status(StatusCode::NOT_FOUND);
        request.get(&format!("/api/iam/groups/{}", foreign_group.id))
            .add_header("Authorization", &auth).await.assert_status(StatusCode::NOT_FOUND);
    }).await;
}

/// Duplicate names and duplicate associations surface as `409 Conflict` from the unique constraints themselves, not from a pre-check.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture tenant is shared across every IAM table's unique-violation case"
)]
async fn duplicate_iam_names_and_associations_conflict() {
    use crate::requests::fixtures::{TenantArgs, TestTenant};
    use serde_json::{Value, json};

    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = format!("Bearer {root}");
        let document = |action: &str| {
            json!({"statements":[{
                "effect":"allow","actions":[action],
                "resources":[{"type":"tenant","tenant_id":tenant.tenant_id}]
            }]})
        };
        let create = |path: &'static str, body: Value| {
            let request = &request;
            let auth = auth.clone();
            async move {
                let response = request
                    .post(path)
                    .add_header("Authorization", auth)
                    .json(&body)
                    .await;
                response.assert_status(StatusCode::CREATED);
                response.json::<Value>()["id"].as_str().unwrap().to_owned()
            }
        };
        let member = request
            .post("/api/members/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":"dup-user","password":"dup-user-password"}))
            .await;
        member.assert_status(StatusCode::CREATED);
        let user_id = member.json::<Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Creating and renaming onto a taken name (policy, group).
        let policy_a = create(
            "/api/iam/policies",
            json!({"name":"dup-policy-a","document":document("group_list")}),
        )
        .await;
        let policy_b = create(
            "/api/iam/policies",
            json!({"name":"dup-policy-b","document":document("policy_list")}),
        )
        .await;
        let group_a = create("/api/iam/groups", json!({"name":"dup-group-a"})).await;
        let group_b = create("/api/iam/groups", json!({"name":"dup-group-b"})).await;
        for (path, body, message) in [(
            "/api/iam/policies",
            json!({"name":"dup-policy-a","document":document("group_list")}),
            "an IAM policy with this name already exists",
        )] {
            let response = request
                .post(path)
                .add_header("Authorization", &auth)
                .json(&body)
                .await;
            response.assert_status(StatusCode::CONFLICT);
            assert!(response.text().contains(message), "{}", response.text());
        }
        for (path, body, message) in [
            (
                format!("/api/iam/policies/{policy_b}"),
                json!({"name":"dup-policy-a","document":document("policy_list")}),
                "an IAM policy with this name already exists",
            ),
            (
                format!("/api/iam/groups/{group_b}"),
                json!({"name":"dup-group-a"}),
                "an IAM group with this name already exists",
            ),
        ] {
            let response = request
                .put(&path)
                .add_header("Authorization", &auth)
                .json(&body)
                .await;
            response.assert_status(StatusCode::CONFLICT);
            assert!(response.text().contains(message), "{}", response.text());
        }

        // Associating twice.
        for (path, message) in [
            (
                format!("/api/iam/groups/{group_a}/members/{user_id}"),
                "group association already exists",
            ),
            (
                format!("/api/iam/groups/{group_a}/policies/{policy_a}"),
                "group association already exists",
            ),
            (
                format!("/api/iam/users/{user_id}/policies/{policy_a}"),
                "policy is already attached to this user",
            ),
        ] {
            request
                .put(&path)
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
            let response = request.put(&path).add_header("Authorization", &auth).await;
            response.assert_status(StatusCode::CONFLICT);
            assert!(
                response.text().contains(message),
                "{path}: {}",
                response.text()
            );
        }
    })
    .await;
}

/// Group member and policy listings map `limit`/`offset` onto the page, on REST and MCP alike.
#[tokio::test]
async fn group_member_and_policy_listings_page_by_limit_and_offset() {
    use crate::requests::fixtures::{TenantArgs, TestTenant};
    use serde_json::{Value, json};

    boot_request_with::<App, _, _>(&[], |request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let root =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = format!("Bearer {root}");
        let group = request
            .post("/api/iam/groups")
            .add_header("Authorization", &auth)
            .json(&json!({"name":"paged-group"}))
            .await;
        group.assert_status(StatusCode::CREATED);
        let group_id = group.json::<Value>()["id"].as_str().unwrap().to_owned();
        for index in 0..3 {
            let name = format!("paged-user-{index}");
            let user = request
                .post("/api/members/users")
                .add_header("Authorization", &auth)
                .json(&json!({"user_name":name,"password":"paged-user-password"}))
                .await;
            user.assert_status(StatusCode::CREATED);
            let user_id = user.json::<Value>()["user_id"].as_str().unwrap().to_owned();
            request
                .put(&format!("/api/iam/groups/{group_id}/members/{user_id}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
            let policy = request
                .post("/api/iam/policies")
                .add_header("Authorization", &auth)
                .json(
                    &json!({"name":format!("paged-policy-{index}"),"document":{"statements":[{
                    "effect":"allow","actions":["group_list"],
                    "resources":[{"type":"tenant","tenant_id":tenant.tenant_id}]}]}}),
                )
                .await;
            policy.assert_status(StatusCode::CREATED);
            let policy_id = policy.json::<Value>()["id"].as_str().unwrap().to_owned();
            request
                .put(&format!("/api/iam/groups/{group_id}/policies/{policy_id}"))
                .add_header("Authorization", &auth)
                .await
                .assert_status(StatusCode::NO_CONTENT);
        }
        let session = crate::controllers::mcp::initialize(&request).await;
        for (path, tool) in [
            ("members", "list_iam_group_members"),
            ("policies", "list_iam_group_policies"),
        ] {
            let fetch = |query: &'static str| {
                let url = format!("/api/iam/groups/{group_id}/{path}{query}");
                let request = &request;
                let auth = &auth;
                async move {
                    let response = request.get(&url).add_header("Authorization", auth).await;
                    response.assert_status_ok();
                    response.json::<Vec<Value>>()
                }
            };
            let all = fetch("").await;
            assert_eq!(all.len(), 3, "{path}");
            for (query, limit, offset, expected) in [
                ("?limit=1&offset=1", 1, 1, &all[1..2]),
                ("?limit=2&offset=2", 2, 2, &all[2..3]),
                ("?limit=2", 2, 0, &all[0..2]),
            ] {
                assert_eq!(fetch(query).await, expected, "REST {path} {query}");
                let paged = crate::controllers::mcp::call_tool(
                    &request,
                    &session,
                    &root,
                    tool,
                    json!({"group_id":group_id,"limit":limit,"offset":offset}),
                )
                .await;
                assert_ne!(paged["result"]["isError"], true, "{paged}");
                assert_eq!(
                    crate::controllers::mcp::tool_result_json(&paged),
                    json!(expected),
                    "MCP {tool} {query}"
                );
            }
        }
    })
    .await;
}
