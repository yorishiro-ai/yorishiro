use super::{boot_request_isolated, with_max_tenants};
use axum::http::StatusCode;
use sea_orm::EntityTrait;
use serial_test::serial;
use yorishiro::App;

pub(crate) async fn iam_mcp_call(
    request: &loco_rs::TestServer,
    session: &str,
    key: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = request.post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session)
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .add_header("Authorization", format!("Bearer {key}"))
        .json(&serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":arguments}})).await;
    response.assert_status_ok();
    let text = response.text();
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or(text.trim());
    serde_json::from_str(payload).expect("MCP JSON response")
}

/// The setup key does not select a workspace implicitly; workspace-scoped keys retain their binding.
#[tokio::test]
#[serial(process_environment)]
async fn iam_management_preserves_credential_boundaries_and_audit_atomicity() {
    with_max_tenants("1", async move {
        boot_request_isolated::<App, _, _>(|request, ctx| async move {
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
            let workspace_id: uuid::Uuid = body["workspace_id"].as_str().unwrap().parse().unwrap();

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
            let user_id = body["user_id"].as_str().unwrap().parse().unwrap();
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

            let initialized = request
                .post("/mcp")
                .add_header("Accept", "application/json, text/event-stream")
                .json(
                    &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                        "protocolVersion":"2025-03-26","capabilities":{},
                        "clientInfo":{"name":"iam-test","version":"1"}
                    }}),
                )
                .await;
            initialized.assert_status_ok();
            let session = initialized.headers()["mcp-session-id"]
                .to_str()
                .unwrap()
                .to_owned();
            let audit_mcp = iam_mcp_call(
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
                iam_mcp_call(&request, &session, &api_key, "create_iam_policy", policy).await;
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

            // An unavailable audit sink must roll back the already-inserted resource.
            use sea_orm::ConnectionTrait;
            ctx.db
                .execute_unprepared(
                    "ALTER TABLE iam_audit_events RENAME TO unavailable_iam_audit_events",
                )
                .await
                .unwrap();
            let failed_rest = request
                .post("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .json(&serde_json::json!({"name":"rest-rollback"}))
                .await;
            let failed_mcp = iam_mcp_call(
                &request,
                &session,
                &api_key,
                "create_iam_group",
                serde_json::json!({"name":"mcp-rollback"}),
            )
            .await;
            ctx.db
                .execute_unprepared(
                    "ALTER TABLE unavailable_iam_audit_events RENAME TO iam_audit_events",
                )
                .await
                .unwrap();
            failed_rest.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(failed_mcp["result"]["isError"], true, "{failed_mcp}");
            let groups = yorishiro::models::iam_groups::Entity::find()
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].name, "operators");

            // Even with one workspace, tenant authentication does not select it.
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
                .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
            let whoami = request
                .get("/api/whoami")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .add_header("x-workspace-id", workspace_id.to_string())
                .await;
            assert_eq!(whoami.status_code(), StatusCode::OK, "{}", whoami.text());
            let identity: serde_json::Value = whoami.json();
            assert_eq!(identity["workspace_id"], workspace_id.to_string());

            // A second workspace can also be selected explicitly.
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

            // A workspace-scoped key acts on its own workspace and refuses a foreign header.
            let user_id: uuid::Uuid = identity["user_id"].as_str().unwrap().parse().unwrap();
            let scoped_key = yorishiro::models::api_keys::Entity::create_api_key(
                &ctx.db,
                workspace_id,
                yorishiro::models::api_keys::ApiKeyScope::Read,
                Some(user_id),
                false,
            )
            .await
            .unwrap();
            let denied = iam_mcp_call(
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
            // A foreign header is a scope denial (403), not a validation failure (422).
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
            let deleted = iam_mcp_call(
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
                user.id,
                yorishiro::models::tenant_memberships::MembershipRole::Viewer,
            )
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
                .await.assert_status(StatusCode::FORBIDDEN);
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
            let denied = iam_mcp_call(&request, &session, &user_key.plaintext,
                "list_iam_policies", serde_json::json!({})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            let attachment = format!("/api/iam/users/{}/policies/{policy_id}", user.id);
            request
                .put(&attachment)
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await
                .assert_status(StatusCode::NO_CONTENT);
            let allowed = iam_mcp_call(
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
            let mcp_inventory = iam_mcp_call(&request, &session, &user_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            let mcp_inventory: serde_json::Value = serde_json::from_str(mcp_inventory["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(inventory, mcp_inventory);
            request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_list = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            assert_eq!(bound_list["result"]["isError"], true, "{bound_list}");
            let detail = request.get(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
            detail.assert_status_ok();
            let mcp_detail = iam_mcp_call(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":workspace_id})).await;
            let mcp_detail: serde_json::Value = serde_json::from_str(mcp_detail["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(detail.json::<serde_json::Value>(), mcp_detail);
            request.get(&format!("/api/workspaces/{second_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_detail = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":second_id})).await;
            assert_eq!(bound_detail["result"]["isError"], true, "{bound_detail}");
            // No legacy workspace grant or selected workspace is needed for an explicit target.
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"IAM workspace"})).await.assert_status_ok();
            let updated_workspace = iam_mcp_call(&request, &session, &user_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":workspace_id,"name":"MCP IAM workspace"})).await;
            assert_ne!(updated_workspace["result"]["isError"], true, "{updated_workspace}");
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"must-not-save","replace_max_entities":true,"max_entities":42}))
                .await.assert_status(StatusCode::FORBIDDEN);
            request.put(&format!("/api/workspaces/{second_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&serde_json::json!({"name":"binding-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let bound = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":second_id,"name":"binding-denied"})).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&template).await.assert_status(StatusCode::CREATED);
            let mut mcp_template = template.clone();
            mcp_template["name"] = serde_json::json!("iam-mcp-template");
            let created_template = iam_mcp_call(&request, &session, &user_key.plaintext,
                "create_template_library_item", mcp_template.clone()).await;
            assert_ne!(created_template["result"]["isError"], true, "{created_template}");
            let created_template: serde_json::Value = serde_json::from_str(
                created_template["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let template_id = created_template["id"].as_str().unwrap();
            request.put(&format!("/api/template-library/{template_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"description":"IAM update"})).await.assert_status_ok();
            let updated = iam_mcp_call(&request, &session, &user_key.plaintext,
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
            let forked = iam_mcp_call(&request, &session, &user_key.plaintext,
                "fork_template_library_item", serde_json::json!({"id":template_id,"name":"iam-mcp-fork"})).await;
            assert_ne!(forked["result"]["isError"], true, "{forked}");
            let forked: serde_json::Value = serde_json::from_str(forked["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let deleted = iam_mcp_call(&request, &session, &user_key.plaintext,
                "delete_template_library_item", serde_json::json!({"id":forked["id"]})).await;
            assert_ne!(deleted["result"]["isError"], true, "{deleted}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&template).await.assert_status(StatusCode::FORBIDDEN);
            let bound = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "create_template_library_item", mcp_template.clone()).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            let detached = iam_mcp_call(
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
            let attached = iam_mcp_call(&request, &session, &api_key, "attach_group_policy",
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
                let mcp = iam_mcp_call(&request, &session, &user_key.plaintext, tool, args).await;
                assert_ne!(mcp["result"]["isError"], true, "{mcp}");
                let mcp: serde_json::Value = serde_json::from_str(mcp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                assert_eq!(rest.json::<serde_json::Value>(), mcp, "{tool}");
            }
            let first = request.get("/api/iam/groups?limit=1")
                .add_header("Authorization", format!("Bearer {api_key}")).await.json::<serde_json::Value>();
            let next = request.get("/api/iam/groups?limit=1&offset=1")
                .add_header("Authorization", format!("Bearer {api_key}")).await.json::<serde_json::Value>();
            assert_eq!(first.as_array().unwrap().len(), 1);
            assert!(next.as_array().unwrap().is_empty());
            // A foreign tenant's resources must not appear even to this tenant's root.
            use sea_orm::{ActiveModelTrait, ActiveValue::Set, TransactionTrait};
            let txn = ctx.db.begin().await.unwrap();
            let foreign_tenant = yorishiro::models::tenant_tenants::ActiveModel {
                name: Set("foreign".into()), ..Default::default()
            }.insert(&txn).await.unwrap();
            let foreign_group = yorishiro::models::iam_groups::ActiveModel {
                tenant_id: Set(foreign_tenant.id), name: Set("foreign".into()), ..Default::default()
            }.insert(&txn).await.unwrap();
            let foreign_workspace = yorishiro::models::workspace_workspaces::ActiveModel {
                tenant_id: Set(foreign_tenant.id), name: Set("foreign".into()),
                status: Set("schema_pending".into()), ..Default::default()
            }.insert(&txn).await.unwrap();
            txn.commit().await.unwrap();
            request.get(&format!("/api/workspaces/{}", foreign_workspace.id))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NOT_FOUND);
            let foreign_detail = iam_mcp_call(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":foreign_workspace.id})).await;
            assert_eq!(foreign_detail["result"]["isError"], true, "{foreign_detail}");
            let inventory = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.json::<serde_json::Value>();
            assert!(inventory.as_array().unwrap().iter().all(|row| row["tenant_id"] == body["tenant_id"]));
            request.get(&format!("/api/iam/groups/{}", foreign_group.id))
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NOT_FOUND);
            let foreign = iam_mcp_call(&request, &session, &api_key, "list_iam_group_members",
                serde_json::json!({"group_id":foreign_group.id})).await;
            assert_eq!(foreign["result"]["isError"], true, "{foreign}");
            let groups = request.get("/api/iam/groups")
                .add_header("Authorization", format!("Bearer {api_key}"))
                .await.json::<serde_json::Value>();
            assert_eq!(groups.as_array().unwrap().len(), 1);
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"IAM created"})).await.assert_status(StatusCode::CREATED);
            let created_workspace = iam_mcp_call(&request, &session, &user_key.plaintext,
                "create_workspace", serde_json::json!({"name":"MCP IAM created"})).await;
            assert_ne!(created_workspace["result"]["isError"], true, "{created_workspace}");
            let created_workspace: serde_json::Value = serde_json::from_str(created_workspace["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            let delete_id = created_workspace["id"].as_str().unwrap();
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let bound_delete = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":delete_id})).await;
            assert_eq!(bound_delete["result"]["isError"], true, "{bound_delete}");
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", scoped_key.plaintext))
                .json(&serde_json::json!({"name":"bound-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let bound = iam_mcp_call(&request, &session, &scoped_key.plaintext,
                "create_workspace", serde_json::json!({"name":"bound-denied"})).await;
            assert_eq!(bound["result"]["isError"], true, "{bound}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"group-allowed"})).await.assert_status(StatusCode::CREATED);
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
            let denied = iam_mcp_call(&request, &session, &user_key.plaintext, "create_iam_group",
                serde_json::json!({"name":"explicit-denied"})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            request.put(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"explicit-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let denied_workspace = iam_mcp_call(&request, &session, &user_key.plaintext,
                "update_workspace", serde_json::json!({"workspace_id":workspace_id,"name":"explicit-denied"})).await;
            assert_eq!(denied_workspace["result"]["isError"], true, "{denied_workspace}");
            request.get(&workspace_path)
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let denied_detail = iam_mcp_call(&request, &session, &user_key.plaintext,
                "get_workspace", serde_json::json!({"workspace_id":workspace_id})).await;
            assert_eq!(denied_detail["result"]["isError"], true, "{denied_detail}");
            let inventory = request.get("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext)).await;
            inventory.assert_status_ok();
            assert!(inventory.json::<Vec<serde_json::Value>>().is_empty());
            let denied_list = iam_mcp_call(&request, &session, &user_key.plaintext,
                "list_workspaces", serde_json::json!({})).await;
            let denied_list: serde_json::Value = serde_json::from_str(denied_list["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(denied_list, serde_json::json!([]));
            request.post("/api/workspaces")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"explicit-denied"})).await.assert_status(StatusCode::FORBIDDEN);
            let denied_create = iam_mcp_call(&request, &session, &user_key.plaintext,
                "create_workspace", serde_json::json!({"name":"explicit-denied"})).await;
            assert_eq!(denied_create["result"]["isError"], true, "{denied_create}");
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::FORBIDDEN);
            let denied_delete = iam_mcp_call(&request, &session, &user_key.plaintext,
                "delete_workspace", serde_json::json!({"workspace_id":delete_id})).await;
            assert_eq!(denied_delete["result"]["isError"], true, "{denied_delete}");
            request.post("/api/template-library")
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&template).await.assert_status(StatusCode::FORBIDDEN);
            let denied_template = iam_mcp_call(&request, &session, &user_key.plaintext,
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
                    let denied = iam_mcp_call(&request, &session, key, tool, args).await;
                    assert_eq!(denied["result"]["isError"], true, "{denied}");
                }
            }
            request.delete(&deny_path).add_header("Authorization", format!("Bearer {api_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            request.delete(&format!("/api/workspaces/{delete_id}"))
                .add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .await.assert_status(StatusCode::NO_CONTENT);
            let deleted_workspace = iam_mcp_call(&request, &session, &user_key.plaintext,
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
            let last = iam_mcp_call(&request, &session, &user_key.plaintext,
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
            let restored = iam_mcp_call(&request, &session, &api_key, "update_iam_policy",
                serde_json::json!({"policy_id":policy_id,"name":"operators","document":policy_result["document"]})).await;
            assert_ne!(restored["result"]["isError"], true, "{restored}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"updated-allowed"})).await.assert_status(StatusCode::CREATED);
            let deleted = iam_mcp_call(&request, &session, &api_key, "delete_iam_policy",
                serde_json::json!({"policy_id":policy_id})).await;
            assert_ne!(deleted["result"]["isError"], true, "{deleted}");
            request.post("/api/iam/groups").add_header("Authorization", format!("Bearer {}", user_key.plaintext))
                .json(&serde_json::json!({"name":"deleted-denied"})).await.assert_status(StatusCode::FORBIDDEN);
        })
        .await;
    })
    .await;
}
