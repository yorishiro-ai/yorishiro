use axum::http::StatusCode;
use chrono::Utc;
use loco_rs::TestServer;
use serde_json::{Value, json};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::{entity_entities, schema_schemas};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

#[tokio::test]
async fn workspace_deletion_reports_fork_references_without_exposing_destination() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ModelTrait, TransactionTrait};
    use yorishiro::models::{_entities::workspace_schema_forks, tenancy};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"workspace-fork", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let txn = ctx.db.begin().await.unwrap();
        let destination = tenancy::create_workspace(&txn, tenant.tenant_id,
            &format!("workspace-fork-{}", Uuid::now_v7()), None, None).await.unwrap();
        let source = schema_schemas::create_schema(&txn, tenant.tenant_id, tenant.workspace_id,
            serde_json::from_value(definition.clone()).unwrap(), None, None).await.unwrap().0;
        let target = schema_schemas::create_schema(&txn, tenant.tenant_id, destination.id,
            serde_json::from_value(definition).unwrap(), None, None).await.unwrap().0;
        let fork = workspace_schema_forks::ActiveModel {
            id: Set(Uuid::now_v7()), tenant_id: Set(tenant.tenant_id), workspace_id: Set(destination.id),
            source_workspace_id: Set(tenant.workspace_id), source_schema_id: Set(source.id),
            source_schema_version: Set(1), source_schema_name: Set("workspace-fork".into()),
            fork_schema_id: Set(target.id), customized: Set(false), ..Default::default()
        }.insert(&txn).await.unwrap();
        txn.commit().await.unwrap();
        let denied = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call",
            json!({"name":"delete_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        assert!(denied.to_string().contains("schema forks"), "{denied}");
        assert!(!denied.to_string().contains(&destination.id.to_string()));
        request.get(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
        fork.delete(&ctx.db).await.unwrap();
        let destination_key = issue_api_key(&ctx, destination.id, tenant.owner_id, ApiKeyScope::Read, false).await;
        let deleted = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call",
            json!({"name":"delete_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        request.get(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {destination_key}")).await.assert_status(StatusCode::NOT_FOUND);
        request.get("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await.assert_status(StatusCode::UNAUTHORIZED);
    }).await;
}

#[tokio::test]
async fn tenant_overview_tool_matches_rest_and_rejects_service_credentials() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let session = initialize(&request).await;
        let overview = mcp_call(
            &request,
            &session,
            &tenant_admin_key,
            2,
            "tools/call",
            json!({"name":"get_tenant_overview","arguments":{}}),
        )
        .await;
        let overview = tool_result_json(&overview);
        let rest = request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {tenant_admin_key}"))
            .await;
        rest.assert_status_ok();
        assert_eq!(overview, rest.json::<Value>());
        assert_eq!(overview["tenant_id"], json!(tenant.tenant_id));
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        let denied = mcp_call(
            &request,
            &session,
            &service.plaintext,
            3,
            "tools/call",
            json!({"name":"get_tenant_overview","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        request
            .get("/api/tenant/overview")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}
#[tokio::test]
async fn schema_fork_tools_match_rest_and_enforce_workspace_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let source = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"template_id":"task-management"})).await;
        source.assert_status(StatusCode::CREATED);
        let source: Value = source.json();
        let target = request.post("/api/workspaces").add_header("Authorization", format!("Bearer {tenant_admin_key}")).json(&json!({"name":"fork target"})).await;
        target.assert_status(StatusCode::CREATED);
        let target: Value = target.json();
        let workspace_id = target["id"].as_str().unwrap().parse().unwrap();
        let key = issue_api_key(&ctx, workspace_id, tenant.owner_id, ApiKeyScope::Schema, false).await;
        let read_key = issue_api_key(&ctx, workspace_id, tenant.owner_id, ApiKeyScope::Read, false).await;
        let input = json!({"source_workspace_id":tenant.workspace_id,"source_schema_id":source["schema"]["id"]});
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"create_schema_fork","arguments":input})).await;
        assert_eq!(denied["result"]["isError"], true);
        let created = mcp_call(&request, &session, &key, 3, "tools/call", json!({"name":"create_schema_fork","arguments":input})).await;
        let fork = tool_result_json(&created);
        let id = fork["id"].as_str().unwrap();
        let rest = request.get(&format!("/api/schema-forks/{id}")).add_header("Authorization", format!("Bearer {key}")).await;
        rest.assert_status_ok();
        assert_eq!(fork, rest.json::<Value>());
        let listed = mcp_call(&request, &session, &read_key, 4, "tools/call", json!({"name":"list_schema_forks","arguments":{}})).await;
        assert_eq!(tool_result_json(&listed), json!([fork.clone()]));
        let foreign = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"get_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(foreign["result"]["isError"], true);
        let stale = mcp_call(&request, &session, &key, 6, "tools/call", json!({"name":"update_schema_fork","arguments":{"fork_id":id,"definition":fork["definition"],"expected_fork_schema_id":Uuid::new_v4()}})).await;
        assert_eq!(stale["result"]["isError"], true);
        let updated = mcp_call(&request, &session, &key, 7, "tools/call", json!({"name":"update_schema_fork","arguments":{"fork_id":id,"definition":fork["definition"],"expected_fork_schema_id":fork["fork_schema_id"]}})).await;
        assert_eq!(tool_result_json(&updated)["id"], fork["id"]);
        let denied = mcp_call(&request, &session, &read_key, 8, "tools/call", json!({"name":"delete_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let deleted = mcp_call(&request, &session, &key, 9, "tools/call", json!({"name":"delete_schema_fork","arguments":{"fork_id":id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        request.get(&format!("/api/schema-forks/{id}")).add_header("Authorization", format!("Bearer {key}")).await.assert_status(StatusCode::NOT_FOUND);
        request.get(&format!("/api/schemas/{}", source["schema"]["id"].as_str().unwrap())).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
    }).await;
}

/// One provider kind's MCP tools: the secret never comes back, REST agrees with MCP, a read key cannot write, and an invalid input leaves the saved value untouched.
async fn provider_tools_keep_secrets_private_and_match_rest(
    kind: &str,
    rest_path: &str,
    input: Value,
    secret: &str,
    shown: (&str, Value),
    invalid: (&str, Value),
) {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let tool = |action: &str| format!("{action}_{kind}_provider");
        let saved = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":tool("set"),"arguments":input}),
        )
        .await;
        assert_eq!(tool_result_json(&saved)["saved"], true);
        assert!(!saved.to_string().contains(secret));
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap()
        .plaintext;
        let current = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({"name":tool("get"),"arguments":{}}),
        )
        .await;
        let metadata = tool_result_json(&current);
        assert!(metadata.get("api_key").is_none());
        assert!(!current.to_string().contains(secret));
        assert_eq!(metadata[shown.0], shown.1);
        let rest = request
            .get(rest_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        rest.assert_status_ok();
        assert_eq!(metadata, rest.json::<Value>());
        for (id, name, arguments) in [
            (4, tool("set"), input.clone()),
            (5, tool("reset"), json!({})),
        ] {
            let denied = mcp_call(
                &request,
                &session,
                &read_key,
                id,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true);
        }
        let mut bad = input;
        bad[invalid.0] = invalid.1;
        let rejected = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            6,
            "tools/call",
            json!({"name":tool("set"),"arguments":bad}),
        )
        .await;
        assert_eq!(rejected["result"]["isError"], true);
        let unchanged = mcp_call(
            &request,
            &session,
            &read_key,
            7,
            "tools/call",
            json!({"name":tool("get"),"arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&unchanged), metadata);
        let reset = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            8,
            "tools/call",
            json!({"name":tool("reset"),"arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request
            .get(rest_path)
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn llm_provider_tools_keep_secrets_private_and_match_rest() {
    provider_tools_keep_secrets_private_and_match_rest(
        "llm",
        "/api/workspace/llm-key",
        json!({"base_url":"https://llm.example.invalid/v1","model":"test-llm","api_key":"test-only-llm-secret"}),
        "test-only-llm-secret",
        ("model", json!("test-llm")),
        ("api_key", json!("")),
    )
    .await;
}

#[tokio::test]
async fn embedding_provider_tools_keep_secrets_private_and_match_rest() {
    provider_tools_keep_secrets_private_and_match_rest(
        "embedding",
        "/api/workspace/embedding-key",
        json!({"base_url":"https://embedding.example.invalid/v1","model":"test-embedding","api_key":"test-only-provider-secret","dimensions":32}),
        "test-only-provider-secret",
        ("dimensions", json!(32)),
        ("dimensions", json!(0)),
    )
    .await;
}

#[tokio::test]
async fn worker_class_tools_match_rest_and_enforce_schema_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let saved = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"set_worker_class","arguments":{"worker_class":"shared"}}),
        )
        .await;
        assert_eq!(tool_result_json(&saved)["saved"], true);
        let current = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"get_worker_class","arguments":{}}),
        )
        .await;
        let rest = request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&current), rest.json::<Value>());
        assert_eq!(tool_result_json(&current)["worker_class"], "shared");
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap()
        .plaintext;
        for (id, name, arguments) in [
            (4, "set_worker_class", json!({"worker_class":"shared"})),
            (5, "reset_worker_class", json!({})),
        ] {
            let denied = mcp_call(
                &request,
                &session,
                &read_key,
                id,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true);
        }
        let reset = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            6,
            "tools/call",
            json!({"name":"reset_worker_class","arguments":{}}),
        )
        .await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request
            .get("/api/workspace/worker-class")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        let absent = mcp_call(
            &request,
            &session,
            &read_key,
            7,
            "tools/call",
            json!({"name":"get_worker_class","arguments":{}}),
        )
        .await;
        assert_eq!(absent["result"]["isError"], true);
    })
    .await;
}

#[tokio::test]
async fn entity_column_tools_match_rest_and_enforce_write_scope() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let saved = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"set_entity_columns","arguments":{"entity_type":"note","columns":["title","body"]}})).await;
        assert_eq!(tool_result_json(&saved), json!({"entity_type":"note","columns":["title","body"]}));
        let listed = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"list_entity_columns","arguments":{}})).await;
        let rest = request.get("/api/workspace/entity-columns").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&listed), rest.json::<Value>());
        let invalid = mcp_call(&request, &session, &tenant.api_key, 4, "tools/call", json!({"name":"set_entity_columns","arguments":{"entity_type":"note","columns":["title","title"]}})).await;
        assert_eq!(invalid["result"]["isError"], true);
        let read_key = yorishiro::models::api_keys::Entity::create_api_key(&ctx.db, tenant.workspace_id, ApiKeyScope::Read, Some(tenant.owner_id), false).await.unwrap().plaintext;
        for (id, name, arguments) in [
            (5, "set_entity_columns", json!({"entity_type":"note","columns":[]})),
            (6, "reset_entity_columns", json!({"entity_type":"note"})),
        ] {
            let denied = mcp_call(&request, &session, &read_key, id, "tools/call", json!({"name":name,"arguments":arguments})).await;
            assert_eq!(denied["result"]["isError"], true);
        }
        let unchanged = mcp_call(&request, &session, &read_key, 7, "tools/call", json!({"name":"list_entity_columns","arguments":{}})).await;
        assert_eq!(tool_result_json(&unchanged), tool_result_json(&listed));
        let reset = mcp_call(&request, &session, &tenant.api_key, 8, "tools/call", json!({"name":"reset_entity_columns","arguments":{"entity_type":"note"}})).await;
        assert_eq!(tool_result_json(&reset)["reset"], true);
        request.get("/api/workspace/entity-columns").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_json(&json!([]));
    }).await;
}

#[tokio::test]
async fn current_user_tools_are_self_only_and_secret_free() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let updated = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":"  Profile  "}}),
        )
        .await;
        let profile = tool_result_json(&updated);
        assert_eq!(profile["id"], tenant.owner_id.to_string());
        assert_eq!(profile["display_name"], "Profile");
        assert!(profile.get("password_hash").is_none());
        assert!(profile.get("oauth_subject_id").is_none());
        let rest = request
            .get("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await;
        rest.assert_status_ok();
        assert_eq!(profile, rest.json::<Value>());
        let invalid = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":" "}}),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
        request
            .put("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"display_name":"Other", "email":"replacement@example.com"}))
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        request
            .put("/api/users/me")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"display_name":null}))
            .await
            .assert_status_ok();
        let current = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            4,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        assert!(tool_result_json(&current)["display_name"].is_null());
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap()
        .plaintext;
        let denied = mcp_call(
            &request,
            &session,
            &service,
            5,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        let denied_update = mcp_call(
            &request,
            &session,
            &service,
            6,
            "tools/call",
            json!({"name":"update_current_user","arguments":{"display_name":"Service"}}),
        )
        .await;
        assert_eq!(denied_update["result"]["isError"], true);
        request
            .get("/api/users/me")
            .add_header("Authorization", format!("Bearer {service}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn schema_archive_preserves_history_and_version_sequence() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"archive-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let created = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        created.assert_status(StatusCode::CREATED);
        let schema = created.json::<Value>()["schema"].clone();
        let id = schema["id"].as_str().unwrap();
        let entity = request.post("/api/entities").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"schema_name":"archive-history","entity_type":"note","data":{"title":"retained"}})).await;
        entity.assert_status(StatusCode::CREATED);
        let entity_id = entity.json::<Value>()["id"].as_str().unwrap().to_owned();
        let read_key = issue_api_key(&ctx, tenant.workspace_id, tenant.owner_id, ApiKeyScope::Read, false).await;
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let archived = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":id}})).await;
        assert_eq!(tool_result_json(&archived)["status"], "archived");
        let retained = request.get(&format!("/api/entities/{entity_id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        retained.assert_status_ok();
        assert_eq!(retained.json::<Value>()["data"]["title"], "retained");
        let again = request.post(&format!("/api/schemas/{id}/archive")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        again.assert_status_ok();
        assert_eq!(again.json::<Value>(), tool_result_json(&archived));
        request.get("/api/schemas/active/archive-history").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::NOT_FOUND);
        request.get(&format!("/api/schemas/{id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status_ok();
        let next = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        next.assert_status(StatusCode::CREATED);
        assert_eq!(next.json::<Value>()["schema"]["version"], 2);
        let missing = mcp_call(&request, &session, &tenant.api_key, 4, "tools/call", json!({"name":"archive_schema","arguments":{"schema_id":Uuid::new_v4()}})).await;
        assert_eq!(missing["result"]["isError"], true);
    }).await;
}

#[tokio::test]
async fn delete_schema_tool_removes_only_unreferenced_archived_versions() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"delete-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let v1 = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        v1.assert_status(StatusCode::CREATED);
        let v1_id = v1.json::<Value>()["schema"]["id"].as_str().unwrap().to_owned();
        let v2 = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        v2.assert_status(StatusCode::CREATED);
        let v2_id = v2.json::<Value>()["schema"]["id"].as_str().unwrap().to_owned();
        let v3 = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        v3.assert_status(StatusCode::CREATED);
        let v3_id = v3.json::<Value>()["schema"]["id"].as_str().unwrap().to_owned();
        let read_key = issue_api_key(&ctx, tenant.workspace_id, tenant.owner_id, ApiKeyScope::Read, false).await;
        let denied = mcp_call(&request, &session, &read_key, 2, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v1_id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let bound = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v1_id}})).await;
        assert_eq!(bound["result"]["isError"], true);
        assert!(bound.to_string().contains("workspaces"));
        let active = mcp_call(&request, &session, &tenant.api_key, 4, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v3_id}})).await;
        assert_eq!(active["result"]["isError"], true);
        assert!(active.to_string().contains("still active"));
        let deleted = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":v2_id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        let listed = mcp_call(&request, &session, &tenant.api_key, 6, "tools/call", json!({"name":"list_schemas","arguments":{}})).await;
        let listed = tool_result_json(&listed);
        let rows = listed.as_array().unwrap();
        assert!(rows.iter().all(|row| row["id"] != json!(v2_id)));
        request.get(&format!("/api/schemas/{v2_id}")).add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::NOT_FOUND);
    }).await;
}

#[tokio::test]
async fn api_key_tool_issues_tenant_scoped_keys_with_the_same_ceilings() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let denied = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"create_api_key","arguments":{"name":"from-bound","tenant_scoped":true}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let issuer = yorishiro::models::api_keys::Entity::create_tenant_api_key(&ctx.db, tenant.tenant_id, ApiKeyScope::Migration, Some(tenant.owner_id), true, "initial-admin").await.unwrap();
        let created = mcp_call(&request, &session, &issuer.plaintext, 3, "tools/call", json!({"name":"create_api_key","arguments":{"name":"tenant-scoped","scope":"write","tenant_scoped":true}})).await;
        let record = tool_result_json(&created);
        assert!(record["workspace_id"].is_null(), "record: {record:?}");
        request.get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", record["full_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::OK);
        assert_eq!(record["scope"], "write");
        assert_eq!(record["effective_scope"], "write");
        let rest = request.post("/api/api-keys").add_header("Authorization", format!("Bearer {}", issuer.plaintext)).json(&json!({"name":"rest-twin","scope":"write","tenant_scoped":true})).await;
        rest.assert_status(StatusCode::CREATED);
        let rest_record = rest.json::<Value>();
        assert!(rest_record["workspace_id"].is_null());
        let revoked = mcp_call(&request, &session, &issuer.plaintext, 4, "tools/call", json!({"name":"revoke_api_key","arguments":{"key_id":record["id"].as_str().unwrap()}})).await;
        assert_eq!(tool_result_json(&revoked)["revoked"], true);
        let missing = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"delete_schema","arguments":{"schema_id":Uuid::new_v4()}})).await;
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing.to_string().contains("was not found"));
    }).await;
}

#[tokio::test]
async fn export_jsonl_tool_matches_the_rest_export() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let definition = json!({"name":"export-history","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let created_schema = request.post("/api/schemas").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&definition).await;
        created_schema.assert_status(StatusCode::CREATED);
        let created = request.post("/api/entities").add_header("Authorization", format!("Bearer {}", tenant.api_key)).json(&json!({"schema_name":"export-history","entity_type":"note","data":{"title":"exported"}})).await;
        created.assert_status(StatusCode::CREATED);
        let entity_id = created.json::<Value>()["id"].as_str().unwrap().to_owned();

        let exported = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"export_jsonl","arguments":{}})).await;
        let text = exported["result"]["content"][0]["text"].as_str().unwrap().to_owned();
        let mut kinds = std::collections::BTreeMap::new();
        let mut entity_rows = Vec::new();
        for line in text.lines() {
            let row: Value = serde_json::from_str(line).expect("export line is JSON");
            *kinds.entry(row["kind"].as_str().unwrap().to_owned()).or_insert(0) += 1;
            if row["record"]["id"] == entity_id {
                entity_rows.push(row);
            }
        }
        assert!(kinds.get("schema").copied().unwrap_or(0) >= 1, "kinds: {kinds:?}");
        assert_eq!(kinds.get("entity").copied().unwrap_or(0), 1, "kinds: {kinds:?}");
        assert_eq!(entity_rows.len(), 1, "kinds: {kinds:?}");
        assert_eq!(entity_rows[0]["record"]["data"]["title"], "exported");

        // The REST endpoint and the tool return the same lines, in the same order.
        let rest = request.get("/api/export.jsonl").add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(text, rest.text());
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        use yorishiro::models::_entities::entity_entities::{Column, Entity};
        Entity::update_many()
            .col_expr(Column::Data, sea_orm::sea_query::Expr::value(json!({"title":"x".repeat(8 * 1024 * 1024)})))
            .filter(Column::Id.eq(entity_id.parse::<Uuid>().unwrap())).exec(&ctx.db).await.unwrap();
        let overflow = mcp_call(&request, &session, &tenant.api_key, 3, "tools/call", json!({"name":"export_jsonl","arguments":{}})).await;
        assert_eq!(overflow["result"]["isError"], true);
        assert!(overflow.to_string().contains("8 MiB"));
    }).await;
}

#[tokio::test]
async fn audit_tool_preserves_independent_grant_and_rest_parity() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let denied = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"list_audit_log","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        let audit_key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            tenant.owner_id,
            ApiKeyScope::Read,
            true,
        )
        .await;
        let listed = mcp_call(
            &request,
            &session,
            &audit_key,
            3,
            "tools/call",
            json!({"name":"list_audit_log","arguments":{"page":1,"page_size":10}}),
        )
        .await;
        let rest = request
            .get("/api/audit-log?page=1&page_size=10")
            .add_header("Authorization", format!("Bearer {audit_key}"))
            .await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&listed), rest.json::<Value>());
        let invalid = mcp_call(
            &request,
            &session,
            &audit_key,
            4,
            "tools/call",
            json!({"name":"list_audit_log","arguments":{"page":u64::MAX}}),
        )
        .await;
        assert_eq!(invalid["result"]["isError"], true);
        for (key, expected) in [(&tenant.api_key, false), (&audit_key, true)] {
            let capabilities = mcp_call(
                &request,
                &session,
                key,
                5,
                "tools/call",
                json!({"name":"capabilities","arguments":{}}),
            )
            .await;
            let capabilities = tool_result_json(&capabilities);
            let audit = capabilities["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "list_audit_log")
                .unwrap();
            assert_eq!(audit["required_grants"], json!(["audit"]));
            assert_eq!(audit["grants_satisfied"], expected);
        }
    })
    .await;
}

pub(super) fn rpc_body(text: &str) -> Value {
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|line| !line.is_empty())
        .unwrap_or(text.trim());
    serde_json::from_str(payload).unwrap_or_else(|err| panic!("invalid MCP response {err}: {text}"))
}

pub(crate) async fn initialize(request: &TestServer) -> String {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "yorishiro-test", "version": "1.0" }
            }
        }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    let _ = rpc_body(&response.text());
    let session = response
        .headers()
        .get("mcp-session-id")
        .expect("MCP session id")
        .to_str()
        .expect("MCP session id is text")
        .to_owned();

    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session.clone())
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::ACCEPTED,
        "response: {}",
        response.text()
    );
    session
}

pub(crate) async fn mcp_call(
    request: &TestServer,
    session: &str,
    api_key: &str,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    let response = request
        .post("/mcp")
        .add_header("Accept", "application/json, text/event-stream")
        .add_header("MCP-Session-Id", session)
        .add_header("MCP-Protocol-Version", "2025-03-26")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .json(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        .await;
    assert_eq!(
        response.status_code(),
        StatusCode::OK,
        "response: {}",
        response.text()
    );
    rpc_body(&response.text())
}

fn tool_inventory(response: &Value) -> Vec<Value> {
    response["result"]["tools"]
        .as_array()
        .expect("tools/list result")
        .clone()
}

pub(crate) fn tool_result_json(response: &Value) -> Value {
    assert_ne!(
        response["result"]["isError"], true,
        "expected a successful tool result: {response}"
    );
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("tool result text"),
    )
    .expect("JSON tool result")
}

#[tokio::test]
async fn tenant_metadata_tools_match_rest_and_keep_updates_atomic() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let updated = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call", json!({"name":"update_tenant","arguments":{"name":" managed tenant ","replace_max_workspaces":true,"max_workspaces":0}})).await;
        let record = tool_result_json(&updated);
        assert_eq!(record["name"], "managed tenant");
        assert_eq!(record["max_workspaces"], 0);
        let rest = request.get("/api/tenants/current").add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
        assert_eq!(record, rest.json::<Value>());
        let blocked = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call", json!({"name":"create_workspace","arguments":{"name":"blocked"}})).await;
        assert_eq!(blocked["result"]["isError"], true);
        for arguments in [json!({"name":"rollback","replace_max_workspaces":true,"max_workspaces":-1}), json!({"name":"rollback","max_workspaces":5}), json!({"name":" "})] {
            let invalid = mcp_call(&request, &session, &tenant_admin_key, 4, "tools/call", json!({"name":"update_tenant","arguments":arguments})).await;
            assert_eq!(invalid["result"]["isError"], true);
        }
        let detail = mcp_call(&request, &session, &tenant_admin_key, 5, "tools/call", json!({"name":"get_tenant","arguments":{}})).await;
        assert_eq!(tool_result_json(&detail), record);
        let cleared = request.put("/api/tenants/current").add_header("Authorization", format!("Bearer {tenant_admin_key}")).json(&json!({"replace_max_workspaces":true,"max_workspaces":null})).await;
        cleared.assert_status_ok();
        assert!(cleared.json::<Value>()["max_workspaces"].is_null());
        let user = yorishiro::models::user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("tenant-viewer"), "test-password", None).await.unwrap();
        yorishiro::models::tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        crate::requests::fixtures::grant_workspace(&ctx, tenant.workspace_id, user.id, yorishiro::models::workspace_memberships::WorkspaceRole::Viewer).await;
        let viewer = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        for (name, args) in [("get_tenant", json!({})), ("update_tenant", json!({"name":"forbidden"}))] {
            let denied = mcp_call(&request, &session, &viewer, 6, "tools/call", json!({"name":name,"arguments":args})).await;
            assert_eq!(denied["result"]["isError"], true);
        }
    }).await;
}

#[tokio::test]
async fn workspace_member_tools_match_rest_and_revoke_bound_credentials() {
    use yorishiro::models::{tenant_memberships, user_users, workspace_memberships::WorkspaceRole};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("workspace-managed"), "test-password", None).await.unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        let session = initialize(&request).await;
        let granted = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"grant_workspace_member","arguments":{"workspace_id":tenant.workspace_id,"user_id":user.id,"role":"admin"}})).await;
        assert_eq!(tool_result_json(&granted)["granted"], true);
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        crate::requests::fixtures::grant_iam_administrator(&txn, tenant.tenant_id, user.id).await;
        txn.commit().await.unwrap();
        let member_key = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        let listed = mcp_call(&request, &session, &member_key, 3, "tools/call", json!({"name":"list_workspace_members","arguments":{"workspace_id":tenant.workspace_id}})).await;
        let rest = request.get(&format!("/api/workspaces/{}/members", tenant.workspace_id)).add_header("Authorization", format!("Bearer {member_key}")).await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&listed), rest.json::<Value>());
        assert_eq!(tool_result_json(&listed), json!([{"user_id":user.id,"role":"admin"}]));
        let tenant_admin = mcp_call(&request, &session, &member_key, 4, "tools/call", json!({"name":"create_api_key","arguments":{"name":"forbidden"}})).await;
        assert_eq!(tenant_admin["result"]["isError"], true);
        crate::requests::fixtures::grant_workspace(&ctx, tenant.workspace_id, user.id, WorkspaceRole::Viewer).await;
        let denied = mcp_call(&request, &session, &member_key, 5, "tools/call", json!({"name":"list_workspace_members","arguments":{"workspace_id":tenant.workspace_id}})).await;
        assert_ne!(denied["result"]["isError"], true);
        let revoked = mcp_call(&request, &session, &tenant.api_key, 7, "tools/call", json!({"name":"revoke_workspace_member","arguments":{"workspace_id":tenant.workspace_id,"user_id":user.id}})).await;
        assert_eq!(tool_result_json(&revoked)["revoked"], true);
        request.get("/api/whoami").add_header("Authorization", format!("Bearer {member_key}")).await.assert_status(StatusCode::UNAUTHORIZED);
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let members = request.get("/api/members").add_header("Authorization", format!("Bearer {administrator_key}")).await.json::<Value>();
        assert!(members.as_array().unwrap().iter().any(|member| member["user_id"] == user.id.to_string()));
    }).await;
}

#[tokio::test]
async fn workspace_member_tools_reject_cross_tenant_grants() {
    use yorishiro::models::{tenant_memberships, user_users};

    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user = user_users::create_user(&ctx.db, &crate::requests::fixtures::unique_email("workspace-managed"), "test-password", None).await.unwrap();
        tenant_memberships::add_member(&ctx.db, tenant.tenant_id, user.id).await.unwrap();
        let session = initialize(&request).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_ne!(tenant.tenant_id, other.tenant_id);
        let foreign = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"grant_workspace_member","arguments":{"workspace_id":other.workspace_id,"user_id":user.id,"role":"admin"}})).await;
        assert_eq!(foreign["result"]["isError"], true);
    }).await;
}

#[tokio::test]
async fn workspace_tools_share_rest_details_updates_and_binding_limits() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        use axum::http::HeaderValue;
        for header in ["x-tenant-id", "x-workspace-id"] {
            let invalid = HeaderValue::from_bytes(b"\xff").unwrap();
            let rest = request.get("/api/whoami")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .add_header(header, invalid.clone()).await;
            assert_eq!(rest.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

            #[cfg(feature = "enterprise")]
            request.get("/api/schemas/upstream-changes")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .add_header(header, invalid.clone()).await
                .assert_status(StatusCode::UNPROCESSABLE_ENTITY);

            for (id, tool) in [(2, "whoami"), (3, "list_schemas"), (4, "list_audit_log")] {
                let response = request.post("/mcp")
                    .add_header("Accept", "application/json, text/event-stream")
                    .add_header("MCP-Session-Id", session.clone())
                    .add_header("MCP-Protocol-Version", "2025-03-26")
                    .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                    .add_header(header, invalid.clone())
                    .json(&json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":tool,"arguments":{}}}))
                    .await;
                let body = rpc_body(&response.text());
                assert_eq!(body["error"]["code"], -32602, "{body}");
                assert!(body["error"]["message"].as_str().unwrap().contains(header));
            }
        }
        let tenant_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let missing = mcp_call(&request, &session, &tenant_key, 20, "tools/call", json!({"name":"list_schemas","arguments":{}})).await;
        assert_eq!(missing["result"]["isError"], true);
        assert!(missing.to_string().contains("x-workspace-id"));
        let tenant_detail = mcp_call(&request, &session, &tenant_key, 21, "tools/call", json!({"name":"get_tenant","arguments":{}})).await;
        assert_ne!(tenant_detail["result"]["isError"], true);
        let detail = mcp_call(&request, &session, &tenant.api_key, 2, "tools/call", json!({"name":"get_workspace","arguments":{"workspace_id":tenant.workspace_id}})).await;
        let rest = request.get(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {}",tenant.api_key)).await;
        assert_eq!(tool_result_json(&detail),rest.json::<Value>());
        let denied_quota = mcp_call(&request, &session, &tenant.api_key, 22, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":tenant.workspace_id,"replace_max_entities":true,"max_entities":42}})).await;
        assert_eq!(denied_quota["result"]["isError"], true);
        let updated = mcp_call(&request, &session, &tenant_key, 3, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":tenant.workspace_id,"name":"renamed","replace_max_entities":true,"max_entities":42}})).await;
        assert_eq!(tool_result_json(&updated)["name"],"renamed");
        let rest = request.get(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {}",tenant.api_key)).await;
        assert_eq!(rest.json::<Value>()["max_entities"],42);
        let rest = request.put(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {tenant_key}")).json(&json!({"replace_max_entities":true,"max_entities":null})).await;
        rest.assert_status_ok();
        assert!(rest.json::<Value>()["max_entities"].is_null());
        let invalid = request.put(&format!("/api/workspaces/{}",tenant.workspace_id)).add_header("Authorization",format!("Bearer {}",tenant.api_key)).json(&json!({"name":"must-not-save","replace_max_entities":true,"max_entities":-1})).await;
        assert!(!invalid.status_code().is_success());
        let created = mcp_call(&request, &session, &tenant_key, 4, "tools/call", json!({"name":"create_workspace","arguments":{"name":"second"}})).await;
        let second = tool_result_json(&created)["id"].as_str().unwrap().to_owned();
        let denied = mcp_call(&request, &session, &tenant.api_key, 5, "tools/call", json!({"name":"get_workspace","arguments":{"workspace_id":second}})).await;
        assert_eq!(denied["result"]["isError"],true);
        let denied = mcp_call(&request, &session, &tenant.api_key, 6, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":second,"name":"retargeted"}})).await;
        assert_eq!(denied["result"]["isError"],true);
        let list = mcp_call(&request, &session, &tenant.api_key, 7, "tools/call", json!({"name":"list_workspaces","arguments":{}})).await;
        let rest = request.get("/api/workspaces").add_header("Authorization",format!("Bearer {}",tenant.api_key)).await;
        rest.assert_status(StatusCode::FORBIDDEN);
        assert_eq!(list["result"]["structuredContent"],rest.json::<Value>());
        let denied = mcp_call(&request, &session, &tenant.api_key, 8, "tools/call", json!({"name":"delete_workspace","arguments":{"workspace_id":second}})).await;
        assert_eq!(denied["result"]["isError"],true);
        let targeted = mcp_call(&request, &session, &tenant_key, 9, "tools/call", json!({"name":"update_workspace","arguments":{"workspace_id":second,"name":"targeted"}})).await;
        assert_eq!(tool_result_json(&targeted)["name"], "targeted");
        let deleted = mcp_call(&request, &session, &tenant_key, 10, "tools/call", json!({"name":"delete_workspace","arguments":{"workspace_id":second}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"],true);
    }).await;
}

#[tokio::test]
async fn template_library_mutations_match_rest_and_remain_tenant_owned() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let session = initialize(&request).await;
        let created = mcp_call(&request, &session, &tenant_admin_key, 2, "tools/call",
            json!({"name":"create_template_library_item","arguments":{"name":format!("mcp-template-{}",Uuid::now_v7()),"definition":{"name":"note","entity_types":{"note":{"fields":{"title":{"type":"string"}}}}}}})).await;
        let created = tool_result_json(&created);
        let id = created["id"].as_str().unwrap();
        let updated = mcp_call(&request, &session, &tenant_admin_key, 3, "tools/call",
            json!({"name":"update_template_library_item","arguments":{"id":id,"description":"updated through MCP"}})).await;
        let rest = request.get(&format!("/api/template-library/{id}")).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
        assert_eq!(tool_result_json(&updated), rest.json::<Value>());
        let fork = mcp_call(&request, &session, &tenant_admin_key, 4, "tools/call",
            json!({"name":"fork_template_library_item","arguments":{"id":id,"name":format!("fork-{}",Uuid::now_v7())}})).await;
        let fork = tool_result_json(&fork);
        assert_ne!(fork["id"], created["id"]);
        let service = yorishiro::models::api_keys::Entity::create_api_key(&ctx.db, tenant.workspace_id, ApiKeyScope::Migration, None, false).await.unwrap();
        let denied = mcp_call(&request, &session, &service.plaintext, 5, "tools/call",
            json!({"name":"delete_template_library_item","arguments":{"id":id}})).await;
        assert_eq!(denied["result"]["isError"], true);
        let deleted = mcp_call(&request, &session, &tenant_admin_key, 6, "tools/call",
            json!({"name":"delete_template_library_item","arguments":{"id":id}})).await;
        assert_eq!(tool_result_json(&deleted)["deleted"], true);
        let rest = request.get(&format!("/api/template-library/{id}")).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status(StatusCode::NOT_FOUND);
        let rest = request.get(&format!("/api/template-library/{}",fork["id"].as_str().unwrap())).add_header("Authorization", format!("Bearer {tenant_admin_key}")).await;
        rest.assert_status_ok();
    }).await;
}
#[tokio::test]
async fn expired_keys_are_rejected_over_rest_and_an_existing_mcp_session() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let session = initialize(&request).await;
        let before = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        assert_eq!(before["result"]["isError"], false);
        let keys = request
            .get("/api/api-keys")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .await
            .json::<Value>();
        let prefix = tenant.api_key.split('_').nth(1).unwrap();
        let id = keys["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|key| {
                key["prefix"]
                    .as_str()
                    .is_some_and(|value| value.ends_with(prefix))
            })
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        yorishiro::models::api_keys::ActiveModel {
            id: Set(id),
            expires_at: Set(Some(
                (Utc::now() - chrono::Duration::seconds(1)).fixed_offset(),
            )),
            ..Default::default()
        }
        .update(&ctx.db)
        .await
        .unwrap();
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let after = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            3,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        assert!(
            after["error"].is_object() || after["result"]["isError"] == true,
            "{after}"
        );
    })
    .await;
}

#[tokio::test]
async fn issued_read_key_reads_but_rejects_valid_mutations_over_both_transports() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        request.post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .json(&json!({"template_id":"task-management"})).await.assert_status(StatusCode::CREATED);
        let created = request.post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .add_header("x-workspace-id", tenant.workspace_id.to_string())
            .json(&json!({"name":"read-transport-contract","scope":"read"})).await;
        assert_eq!(created.status_code(), StatusCode::CREATED);
        let key = created.json::<Value>()["full_key"].as_str().unwrap().to_owned();
        request.get("/api/entities").add_query_param("schema_name", "task-management")
            .add_header("Authorization", format!("Bearer {key}"))
            .await.assert_status(StatusCode::OK);
        let session = initialize(&request).await;
        let read = mcp_call(&request, &session, &key, 2, "tools/call",
            json!({"name":"list_entities","arguments":{"schema_name":"task-management"}})).await;
        assert_ne!(read["result"]["isError"], true, "{read}");
        let template = mcp_call(&request, &session, &key, 20, "tools/call",
            json!({"name":"get_template","arguments":{"template_id":"general-notes"}})).await;
        assert_ne!(template["result"]["isError"], true, "{template}");
        let rest = request.get("/api/templates/general-notes")
            .add_header("Authorization", format!("Bearer {key}")) .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(tool_result_json(&template), rest.json::<Value>());
        let missing = mcp_call(&request, &session, &key, 21, "tools/call",
            json!({"name":"get_template","arguments":{"template_id":"missing"}})).await;
        assert_eq!(missing["result"]["isError"], true, "{missing}");
        let entity = json!({"schema_name":"task-management","entity_type":"task","data":{"title":"denied task"}});
        let schema = json!({"template_id":"general-notes"});
        let migration = json!({"schema_name":"task-management"});
        for (path, body) in [("/api/entities", &entity), ("/api/schemas", &schema), ("/api/migration-jobs/fill-defaults", &migration)] {
            request.post(path).add_header("Authorization", format!("Bearer {key}"))
                .json(body).await.assert_status(StatusCode::FORBIDDEN);
        }
        for (id, name, arguments, scope) in [
            (3, "create_entity", entity, "write"),
            (4, "create_schema", schema, "schema"),
            (5, "fill_defaults", migration, "migration"),
        ] {
            let denied = mcp_call(&request, &session, &key, id, "tools/call", json!({"name":name,"arguments":arguments})).await;
            assert_eq!(denied["result"]["isError"], true, "{denied}");
            assert_eq!(denied["result"]["structuredContent"]["error"]["required_scope"], scope, "{denied}");
        }
    }).await;
}

#[tokio::test]
async fn mcp_whoami_matches_rest_identity_without_secrets() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = issue_api_key(
            &ctx,
            tenant.workspace_id,
            tenant.owner_id,
            ApiKeyScope::Read,
            true,
        )
        .await;
        let session = initialize(&request).await;
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &key,
            2,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["scope"], "read");
        assert_eq!(identity["audit"], true);
        assert_eq!(identity["identity_kind"], "user");
        assert!(identity.get("workspace_access").is_none());
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["bound_workspace_id"], tenant.workspace_id.to_string());
        assert!(!identity["workspace_name"].as_str().unwrap().is_empty());
        assert!(!identity["tenant_name"].as_str().unwrap().is_empty());
        assert!(!result.to_string().contains(&key));
        assert!(!result.to_string().contains("key_hash"));
        let grants = mcp_call(
            &request,
            &session,
            &key,
            3,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        assert_eq!(grants["scopes"], json!(["read"]));
        assert_eq!(grants["audit"], true);
        assert_eq!(grants["tenant_root"], true);
        let postgres = ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Postgres;
        assert_eq!(grants["constraints"]["database_backend"], if postgres { "postgres" } else { "sqlite" });
        assert_eq!(grants["constraints"]["row_level_security"], postgres);
        assert_eq!(grants["constraints"]["workspace_key_retargeting"], false);
        assert_eq!(grants["constraints"]["tenant_key_target_header"], "X-Workspace-Id");
        assert_eq!(grants["constraints"]["discovery_authorizes_requests"], false);
        let inventory = mcp_call(&request, &session, &key, 4, "tools/list", json!({})).await;
        let tools = grants["tools"].as_array().expect("tool capabilities");
        let inventory = tool_inventory(&inventory);
        assert_eq!(tools.len(), inventory.len());
        for tool in &inventory {
            let capability = tools.iter().find(|entry| entry["name"] == tool["name"])
                .expect("every visible tool has a capability");
            assert_eq!(capability["required_scope"], tool["_meta"]["yorishiro/required_scope"]);
            assert_eq!(capability["scope_satisfied"], capability["required_scope"] == "read");
        }
        let denied = mcp_call(&request, &session, &key, 5, "tools/call",
            json!({"name":"create_entity","arguments":{"schema_name":"missing","entity_type":"missing","data":{}}})).await;
        let create = tools.iter().find(|entry| entry["name"] == "create_entity").unwrap();
        assert_eq!(create["required_scope"], denied["result"]["structuredContent"]["error"]["required_scope"]);
    })
    .await;
}

#[tokio::test]
async fn viewer_discovery_agrees_across_login_rest_and_mcp() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::workspace_memberships::WorkspaceRole;

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let sibling = request
            .post("/api/workspaces")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .json(&json!({"name":format!("private-{}", Uuid::new_v4())}))
            .await;
        assert_eq!(sibling.status_code(), StatusCode::CREATED);
        let sibling_id = sibling.json::<Value>()["id"].as_str().unwrap().to_owned();
        let txn = ctx.db.begin().await.unwrap();
        let user = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &crate::requests::fixtures::unique_email("viewer-discovery"),
            "hunter2-hunter2",
            None)
        .await
        .unwrap();
        yorishiro::models::workspace_memberships::Model::grant(
            &txn,
            tenant.workspace_id,
            user.id,
            WorkspaceRole::Viewer,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .json(&json!({"name":"viewer-discovery", "document":{"statements":[{
                "effect":"allow", "actions":["workspace_list"],
                "resources":[{"type":"tenant","tenant_id":tenant.tenant_id}]
            }, {
                "effect":"allow", "actions":["workspace_get"],
                "resources":[{"type":"workspace","tenant_id":tenant.tenant_id,"workspace_id":tenant.workspace_id}]
            }]}})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<Value>()["id"].as_str().unwrap().to_owned();
        request.put(&format!("/api/iam/users/{}/policies/{policy_id}", user.id))
            .add_header("Authorization", format!("Bearer {administrator_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let key = issue_api_key(&ctx, tenant.workspace_id, user.id, ApiKeyScope::Read, false).await;
        let discovery_key = yorishiro::models::api_keys::Entity::create_tenant_api_key(
            &ctx.db, tenant.tenant_id, ApiKeyScope::Read, Some(user.id), false, "discovery",
        ).await.unwrap().plaintext;
        let session = initialize(&request).await;
        let discovered = mcp_call(
            &request,
            &session,
            &key,
            2,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        assert_ne!(discovered["result"]["isError"], true, "{discovered}");
        let candidates = tool_result_json(&discovered);
        assert_eq!(candidates.as_array().unwrap().len(), 1);
        assert_eq!(
            candidates[0]["workspace_id"],
            tenant.workspace_id.to_string()
        );
        let listed = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {discovery_key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed = listed.json::<Value>();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], candidates[0]["workspace_id"]);
        request
            .get(&format!("/api/workspaces/{sibling_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        let login = request
            .post("/auth/root/login")
            .json(&json!({"email":user.email,"password":"hunter2-hunter2"}))
            .await;
        assert_eq!(login.status_code(), StatusCode::OK, "{}", login.text());
        assert_eq!(
            login.json::<Value>()["tenant_id"],
            tenant.tenant_id.to_string()
        );
        let identity = mcp_call(
            &request,
            &session,
            &key,
            3,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let identity = tool_result_json(&identity);
        assert_eq!(identity, rest.json::<Value>());
        assert!(identity.get("workspace_access").is_none());
        assert!(!candidates.to_string().contains(&sibling_id));
    })
    .await;
}

#[tokio::test]
async fn mcp_workspace_discovery_respects_credential_tenant_and_service_identity() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let session = initialize(&request).await;
        let response = mcp_call(
            &request,
            &session,
            &tenant.api_key,
            2,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        let workspaces = tool_result_json(&response);
        let candidates = workspaces.as_array().unwrap();
        assert!(
            candidates
                .iter()
                .any(|workspace| workspace["workspace_id"] == tenant.workspace_id.to_string())
        );
        assert!(
            candidates
                .iter()
                .all(|workspace| workspace["tenant_id"] == tenant.tenant_id.to_string())
        );
        let service = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            ApiKeyScope::Read,
            None,
            false,
        )
        .await
        .unwrap();
        let response = mcp_call(
            &request,
            &session,
            &service.plaintext,
            3,
            "tools/call",
            json!({"name":"discover_workspaces","arguments":{}}),
        )
        .await;
        let workspaces = tool_result_json(&response);
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(
            workspaces[0]["workspace_id"],
            tenant.workspace_id.to_string()
        );
        assert!(!response.to_string().contains(&service.plaintext));
        let grants = mcp_call(
            &request,
            &session,
            &service.plaintext,
            4,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        assert_eq!(grants["scopes"], json!(["read"]));
        assert_eq!(grants["audit"], false);
        assert_eq!(grants["tenant_root"], false);
        let rest = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await;
        assert_eq!(rest.status_code(), StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &service.plaintext,
            5,
            "tools/call",
            json!({"name":"whoami","arguments":{}}),
        )
        .await;
        let identity = tool_result_json(&result);
        assert_eq!(identity, rest.json::<Value>());
        assert_eq!(identity["identity_kind"], "service");
        assert!(identity.get("workspace_access").is_none());
        assert!(identity["user_id"].is_null());
        assert_eq!(identity["stored_scope"], "read");
        assert_eq!(identity["scope"], "read");
        assert_eq!(
            identity["bound_workspace_id"],
            tenant.workspace_id.to_string()
        );
        assert!(!result.to_string().contains(&service.plaintext));
    })
    .await;
}
#[tokio::test]
async fn mcp_authentication_database_failure_is_redacted() {
    if !crate::require_sqlite_backend() {
        return;
    }
    use sea_orm::ConnectionTrait;

    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.db
            .execute_unprepared("ALTER TABLE api_keys RENAME TO private_auth_diagnostic")
            .await
            .expect("break authentication lookup in this isolated test database");
        let response = mcp_call(
            &request,
            &session,
            "not-a-real-key",
            2,
            "tools/call",
            json!({"name": "list_entities", "arguments": {}}),
        )
        .await;
        assert_eq!(response["error"]["message"], "internal server error");
        assert!(!response.to_string().contains("api_keys"));
        assert!(!response.to_string().contains("private_auth_diagnostic"));
    })
    .await;
}

#[tokio::test]
async fn mcp_authenticator_permission_denials_are_structured_not_internal_errors() {
    use std::sync::Arc;
    use yorishiro::controllers::middleware::auth::Authenticator;

    struct PermissionDenied;
    #[loco_rs::prelude::async_trait]
    impl Authenticator for PermissionDenied {
        async fn authenticate(
            &self,
            _app: &loco_rs::app::AppContext,
            _key: &str,
            _headers: &[(String, String)],
        ) -> Result<yorishiro::models::api_keys::AuthContext, yorishiro::YorishiroError> {
            Err(yorishiro::YorishiroError::ScopeInsufficient {
                message: "workspace access denied".into(),
                hint: "request a workspace grant".into(),
            })
        }
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let session = initialize(&request).await;
        ctx.shared_store
            .insert(Arc::new(PermissionDenied) as Arc<dyn Authenticator>);
        for (id, name) in [(2, "list_entities"), (3, "whoami")] {
            let result = mcp_call(
                &request,
                &session,
                "denied-key",
                id,
                "tools/call",
                json!({"name":name,"arguments":{}}),
            )
            .await;
            assert_eq!(result["result"]["isError"], true, "{result}");
            let error = &result["result"]["structuredContent"]["error"];
            assert_eq!(error["code"], "scope_insufficient");
            assert_eq!(error["message"], "workspace access denied");
            assert_eq!(error["hint"], "request a workspace grant");
            assert!(error.get("required_scope").is_none());
            assert!(error.get("effective_scope").is_none());
        }
    })
    .await;
}

/// A server instance is retained by rmcp for the whole session.
/// Origin operations remain ungated, like their REST counterparts, in an existing session.
#[tokio::test]
async fn mcp_origin_tools_match_ungated_rest_when_same_session_licence_state_is_missing() {
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let TestTenant {
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Schema, false).await;
        let session = initialize(&request).await;

        let licensed = mcp_call(&request, &session, &key, 2, "tools/list", json!({})).await;
        let licensed_tools = tool_inventory(&licensed);
        for name in ["list_upstream_changes", "merge_preview", "merge_apply"] {
            assert!(licensed_tools.iter().any(|tool| tool["name"] == name));
        }
        for tool in &licensed_tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }

        // Inject missing process state without manufacturing a licence or replacing its verifier.
        assert!(
            ctx.shared_store
                .remove::<std::sync::Arc<LicenceState>>()
                .is_some()
        );
        let community = mcp_call(&request, &session, &key, 3, "tools/list", json!({})).await;
        let community_tools = tool_inventory(&community);
        let grants = mcp_call(
            &request,
            &session,
            &key,
            4,
            "tools/call",
            json!({"name":"capabilities","arguments":{}}),
        )
        .await;
        let grants = tool_result_json(&grants);
        let capabilities = grants["tools"]
            .as_array()
            .expect("visible tool capabilities");
        assert_eq!(capabilities.len(), community_tools.len());
        for name in ["list_upstream_changes", "merge_preview", "merge_apply"] {
            assert!(community_tools.iter().any(|tool| tool["name"] == name));
            assert!(capabilities.iter().any(|tool| tool["name"] == name));
        }
        let rest = request
            .get("/api/schemas/upstream-changes")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        let result = mcp_call(
            &request,
            &session,
            &key,
            10,
            "tools/call",
            json!({"name":"list_upstream_changes","arguments":{}}),
        )
        .await;
        assert_ne!(result["result"]["isError"], true, "{result}");
        assert_eq!(tool_result_json(&result), rest.json::<Value>());
    })
    .await;
}

/// Origin tools read templates through the control-plane connection, keep schema work on an
/// RLS-scoped transaction, enforce read/schema scopes, and explicitly commit a successful merge.
#[tokio::test]
async fn origin_mcp_tools_enforce_scope_and_commit_the_merge() {
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            api_key: schema_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;

        let library_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant_id, owner_id).await;
        let original = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        });
        let template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {library_key}"))
            .json(&json!({ "name": "library-note", "definition": original }))
            .await;
        assert_eq!(
            template.status_code(),
            StatusCode::CREATED,
            "{}",
            template.text()
        );
        let template_id: Uuid = template.json::<Value>()["id"]
            .as_str()
            .expect("template id")
            .parse()
            .expect("template UUID");

        let create = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {schema_key}"))
            .json(&json!({ "template_id": template_id.to_string() }))
            .await;
        assert_eq!(
            create.status_code(),
            StatusCode::CREATED,
            "response: {}",
            create.text()
        );
        let schema_id: Uuid = create.json::<Value>()["schema"]["id"]
            .as_str()
            .expect("schema id")
            .parse()
            .expect("schema UUID");

        let updated = json!({
            "name": "library-note",
            "entity_types": {
                "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "category": { "type": "string" }
                } }
            }
        });
        let update = request
            .put(&format!("/api/template-library/{template_id}"))
            .add_header("Authorization", format!("Bearer {library_key}"))
            .json(&json!({ "definition": updated }))
            .await;
        assert_eq!(update.status_code(), StatusCode::OK, "{}", update.text());

        let session = initialize(&request).await;
        let changes = mcp_call(
            &request,
            &session,
            &read_key,
            2,
            "tools/call",
            json!({
                "name": "list_upstream_changes",
                "arguments": { "limit": 10, "offset": 0 }
            }),
        )
        .await;
        assert_eq!(changes["result"]["isError"], false, "MCP call: {changes}");
        let changes = tool_result_json(&changes);
        assert_eq!(changes.as_array().expect("change list").len(), 1);

        let preview = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({
                "name": "merge_preview",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(preview["result"]["isError"], false, "MCP call: {preview}");
        assert_eq!(tool_result_json(&preview)["summary"]["auto_add"], 1);

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            4,
            "tools/call",
            json!({
                "name": "merge_apply",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true, "MCP call: {denied}");
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["code"],
            "scope_insufficient"
        );
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["effective_scope"],
            "read"
        );
        assert_eq!(
            schema_schemas::get_active_schema(&ctx.db, workspace_id, "library-note")
                .await
                .expect("active schema after denial")
                .version,
            1
        );

        let merged = mcp_call(
            &request,
            &session,
            &schema_key,
            5,
            "tools/call",
            json!({
                "name": "merge_apply",
                "arguments": { "schema_id": schema_id }
            }),
        )
        .await;
        assert_eq!(merged["result"]["isError"], false, "MCP call: {merged}");
        assert_eq!(tool_result_json(&merged)["schema"]["version"], 2);

        let persisted = schema_schemas::get_active_schema(&ctx.db, workspace_id, "library-note")
            .await
            .expect("persisted merged schema");
        assert_eq!(persisted.version, 2);
        assert!(
            persisted.definition.entity_types["note"]
                .fields
                .contains_key("category")
        );
    })
    .await;
}

/// The fill_defaults MCP path must be discoverable, execute with Migration scope, persist its
/// writes and snapshot, and leave an audit record. A lower-scope key receives an isError result
/// and cannot change the entity.
#[tokio::test]
async fn fill_defaults_mcp_executes_and_enforces_migration_scope() {
    use sea_orm::EntityTrait;
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            api_key: migration_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Migration,
                ..Default::default()
            },
        )
        .await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let audit_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, true).await;

        let v1 = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
            }
        }))
        .expect("parse v1 definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, v1, None, None)
            .await
            .expect("create v1 schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: json!({ "title": "before" }),
            },
            None,
        )
        .await
        .expect("create entity on v1");
        let v2 = serde_json::from_value(serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": {
                    "title": { "type": "string", "required": true },
                    "priority": { "type": "integer", "required": true, "default": 7 }
                } }
            }
        }))
        .expect("parse v2 definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, v2, None, None)
            .await
            .expect("create v2 schema");

        let session = initialize(&request).await;
        let tools = mcp_call(&request, &session, &read_key, 2, "tools/list", json!({})).await;
        assert!(
            tools["result"]["tools"]
                .as_array()
                .expect("tools list")
                .iter()
                .any(|tool| tool["name"] == "fill_defaults")
        );

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({ "name": "fill_defaults", "arguments": { "schema_name": "note" } }),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["required_scope"],
            "migration"
        );
        assert!(
            denied["result"]["content"][0]["text"]
                .as_str()
                .expect("denial text")
                .contains("scope")
        );
        let unchanged = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read denied entity");
        assert_eq!(unchanged.data, json!({ "title": "before" }));

        let call = mcp_call(
            &request,
            &session,
            &migration_key,
            4,
            "tools/call",
            json!({ "name": "fill_defaults", "arguments": { "schema_name": "note" } }),
        )
        .await;
        assert_eq!(call["result"]["isError"], false, "MCP call: {call}");
        let report: Value = serde_json::from_str(
            call["result"]["content"][0]["text"]
                .as_str()
                .expect("report text"),
        )
        .expect("serialized fill_defaults report");
        assert_eq!(report["schema_name"], "note");
        assert!(report["job_id"].as_str().is_some());
        assert!(report.get("entities_updated").is_none());
        let job_id: Uuid = report["job_id"].as_str().unwrap().parse().unwrap();
        let pending = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .unwrap();
        assert_eq!(pending.data, json!({"title": "before"}));
        crate::requests::fill_defaults::finish_via_api(
            &request,
            &ctx,
            job_id,
            &migration_key,
            &format!("/api/migration-jobs/{job_id}"),
        )
        .await;

        let updated = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read filled entity");
        assert_eq!(updated.data, json!({ "title": "before", "priority": 7 }));
        assert_eq!(updated.schema_version, 1);
        let job_id: Uuid = report["job_id"].as_str().unwrap().parse().unwrap();

        for (index, name) in ["migration_job_status", "undo_migration_job"]
            .iter()
            .enumerate()
        {
            let denied = mcp_call(
                &request,
                &session,
                &read_key,
                10 + index as i64,
                "tools/call",
                json!({"name":name,"arguments":{"job_id":job_id}}),
            )
            .await;
            assert_eq!(denied["result"]["isError"], true);
            assert_eq!(
                denied["result"]["structuredContent"]["error"]["required_scope"],
                "migration"
            );
        }
        let status = mcp_call(
            &request,
            &session,
            &migration_key,
            12,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":job_id}}),
        )
        .await;
        assert_eq!(status["result"]["isError"], false);
        let status: Value =
            serde_json::from_str(status["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        let rest = request
            .get(&format!("/api/migration-jobs/{job_id}"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(status, rest.json::<Value>());
        let missing = mcp_call(
            &request,
            &session,
            &migration_key,
            13,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":Uuid::new_v4()}}),
        )
        .await;
        assert_eq!(missing["result"]["isError"], true);
        let undo = mcp_call(
            &request,
            &session,
            &migration_key,
            14,
            "tools/call",
            json!({"name":"undo_migration_job","arguments":{"job_id":job_id}}),
        )
        .await;
        assert_eq!(undo["result"]["isError"], false, "{undo}");
        let undo: Value =
            serde_json::from_str(undo["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(undo["snapshot_job_id"], job_id.to_string());
        let undo_id = undo["job_id"].as_str().unwrap().parse().unwrap();
        crate::requests::fill_defaults::finish_via_api(
            &request,
            &ctx,
            undo_id,
            &migration_key,
            &format!("/api/migration-jobs/{undo_id}"),
        )
        .await;
        let restored = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("read undone entity");
        assert_eq!(restored.data, json!({ "title": "before" }));
        assert_eq!(restored.schema_version, 1);

        let denied = mcp_call(
            &request,
            &session,
            &read_key,
            15,
            "tools/call",
            json!({"name":"reindex_workspace","arguments":{}}),
        )
        .await;
        assert_eq!(denied["result"]["isError"], true);
        assert_eq!(
            denied["result"]["structuredContent"]["error"]["required_scope"],
            "migration"
        );
        let reindex = mcp_call(
            &request,
            &session,
            &migration_key,
            16,
            "tools/call",
            json!({"name":"reindex_workspace","arguments":{}}),
        )
        .await;
        assert_eq!(reindex["result"]["isError"], false, "{reindex}");
        let reindex = tool_result_json(&reindex);
        let reindex_id: Uuid = reindex["job_id"].as_str().unwrap().parse().unwrap();
        let outbox =
            yorishiro::models::_entities::queue_job_dispatch_outbox::Entity::find_by_id(reindex_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("committed reindex dispatch");
        assert_eq!(outbox.workspace_id, Some(workspace_id));
        let status = mcp_call(
            &request,
            &session,
            &migration_key,
            17,
            "tools/call",
            json!({"name":"migration_job_status","arguments":{"job_id":reindex_id}}),
        )
        .await;
        assert_eq!(status["result"]["isError"], false, "{status}");
        let status = tool_result_json(&status);
        assert_eq!(status["job_name"], "reindex");
        let rest = request
            .get(&format!("/api/migration-jobs/{reindex_id}"))
            .add_header("Authorization", format!("Bearer {migration_key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(rest.json::<Value>()["job_name"], status["job_name"]);
        assert_eq!(rest.json::<Value>()["job_id"], status["job_id"]);

        let audit = request
            .get("/api/audit-log")
            .add_header("Authorization", format!("Bearer {audit_key}"))
            .await;
        assert_eq!(
            audit.status_code(),
            StatusCode::OK,
            "response: {}",
            audit.text()
        );
        let entries: Vec<Value> = audit.json();
        assert!(
            entries
                .iter()
                .any(|entry| entry["action"] == "reindex_embeddings")
        );
        let fill_entry = entries
            .iter()
            .find(|entry| entry["action"] == "fill_defaults")
            .expect("fill_defaults audit record");
        assert_eq!(fill_entry["detail"]["schema_name"], "note");
        assert_eq!(fill_entry["detail"]["job_id"], job_id.to_string());
        assert_eq!(fill_entry["detail"]["entities_updated"], 1);
        assert_eq!(fill_entry["detail"]["fields_filled"], 1);
    })
    .await;
}

#[tokio::test]
async fn mcp_import_rolls_back_prior_records_when_a_later_line_is_invalid() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            api_key: schema_key,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let session = initialize(&request).await;
        let schema_id = Uuid::new_v4();
        let jsonl = format!(
            "{}\nnot-json\n",
            json!({
                "kind": "schema",
                "record": {
                    "id": schema_id,
                    "tenant_id": tenant_id,
                    "workspace_id": workspace_id,
                    "name": "rollback-schema",
                    "version": 1,
                    "definition": {
                        "name": "rollback-schema",
                        "entity_types": { "note": { "fields": {} } },
                        "relation_types": {}
                    },
                    "status": "active",
                    "origin_template_id": null,
                    "origin_status": "detached",
                    "origin_snapshot": null,
                    "origin_updated_at": null,
                    "created_at": Utc::now()
                }
            })
        );
        let result = mcp_call(
            &request,
            &session,
            &schema_key,
            2,
            "tools/call",
            json!({ "name": "import_jsonl", "arguments": { "jsonl": jsonl } }),
        )
        .await;
        let accepted = tool_result_json(&result);
        let id = accepted["job_id"].as_str().unwrap().parse().unwrap();
        crate::requests::fill_defaults::fail_via_api(
            &request,
            &ctx,
            id,
            &schema_key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;
        let status = mcp_call(
            &request,
            &session,
            &schema_key,
            3,
            "tools/call",
            json!({"name": "import_job_status", "arguments": {"job_id": id}}),
        )
        .await;
        assert_eq!(tool_result_json(&status)["status"], "failed");
        assert!(
            schema_schemas::get_active_schema(&ctx.db, workspace_id, "rollback-schema")
                .await
                .is_err()
        );
    })
    .await;
}

/// High-dimensional HNSW retains the nearest original cosine through both transports.
#[tokio::test]
async fn postgres_search_returns_the_nearest_document_across_code_collapse_and_recovery() {
    use crate::requests::query_worker::boot_with_query_worker;
    use sea_orm::{ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
    use yorishiro::models::{_entities::entity_embeddings as vectors, entity_embeddings};

    if !crate::require_postgres_backend() {
        return;
    }
    let (provider, _server, _) = crate::http_stub::keyword_provider(4000).await;
    boot_with_query_worker(provider.clone(), |request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = serde_json::from_value(json!({
            "name":"note", "entity_types":{"note":{"fields":{
                "title":{"type":"string", "required":true, "x-embed":true}
            }}}
        }))
        .unwrap();
        schema_schemas::create_schema(
            &ctx.db,
            setup.tenant_id,
            setup.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .unwrap();
        let nearest = entity_entities::create(
            &ctx.db,
            setup.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: json!({"title":"nearest document"}),
            },
            None,
        )
        .await
        .unwrap();
        entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            setup.workspace_id,
            &nearest,
            provider.as_ref(),
        )
        .await
        .unwrap();
        let mut embeddings = Vec::new();
        // Exceed the former candidate cap to protect against reintroducing candidate loss.
        for i in 0..2000 {
            let document = entity_entities::create(
                &ctx.db,
                setup.workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: json!({"title":format!("decoy {i}")}),
                },
                None,
            )
            .await
            .unwrap();
            embeddings.push(vectors::ActiveModel {
                workspace_id: Set(setup.workspace_id),
                entity_id: Set(document.id),
                tenant_id: Set(setup.tenant_id),
                // Distinct zero-mean decoys: a corpus of identical vectors saturates every HNSW
                // neighbour list with zero-distance links, which no approximate index can traverse.
                embedding: Set(Some(sea_orm::entity::prelude::PgVector::from(
                    (0..4000_usize)
                        .map(|axis| {
                            ((i + 1) * (axis + 11) * 7919 % 104_729) as f32 / 104_729.0 - 0.5
                        })
                        .collect::<Vec<_>>(),
                ))),
            });
        }
        vectors::Entity::insert_many(embeddings)
            .exec(&ctx.db)
            .await
            .unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db)
            .await
            .unwrap();
        let session = initialize(&request).await;
        for (phase, background) in [(0, 0.001_f32), (1, -0.001_f32)] {
            let mut vector = vec![background; 4000];
            vector[1] = 1.0;
            vectors::Entity::update_many()
                .col_expr(
                    vectors::Column::Embedding,
                    sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(vector)),
                )
                .filter(vectors::Column::EntityId.eq(nearest.id))
                .exec(&ctx.db)
                .await
                .unwrap();
            let rest = request
                .get("/api/search?query_text=beta&limit=10")
                .add_header("Authorization", format!("Bearer {}", setup.api_key));
            let mcp = mcp_call(
                &request,
                &session,
                &setup.api_key,
                10 + phase,
                "tools/call",
                json!({"name":"search_entities","arguments":{"query_text":"beta","limit":10}}),
            );
            // Concurrent transports share preparation but retain independent authorization and ranking.
            let (rest, mcp) = tokio::join!(async { rest.await }, mcp);
            rest.assert_status(StatusCode::OK);
            let hits = rest.json::<Value>();
            assert_eq!(hits.as_array().unwrap().len(), 10);
            assert_eq!(
                hits[0]["entity"]["id"],
                nearest.id.to_string(),
                "phase {phase}: {hits}"
            );
            assert_eq!(mcp["result"]["isError"], false, "{mcp}");
            assert_eq!(
                tool_result_json(&mcp),
                hits,
                "phase {phase}: transport results agree"
            );
        }
    })
    .await;
}

/// The MCP search tool takes the same path as `GET /api/search`: the server queues the query and a worker holding the model embeds it.
#[tokio::test]
async fn mcp_search_asks_a_worker_to_embed_the_query() {
    use crate::http_stub::{keyword_provider, unavailable_provider};
    use crate::requests::query_worker::boot_with_query_worker;
    use yorishiro::models::entity_embeddings;

    let (provider, _server, inputs) = keyword_provider(4096).await;
    boot_with_query_worker(provider.clone(), |request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let read_key = issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let definition = serde_json::from_value(json!({
            "name": "note",
            "entity_types": { "note": { "fields": {
                "title": { "type": "string", "required": true, "x-embed": true }
            } } }
        }))
        .expect("definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, definition, None, None)
            .await
            .expect("create schema");
        let mut candidates = Vec::new();
        for title in ["alpha plan", "beta plan"] {
            let record = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: json!({ "title": title }),
                },
                None,
            )
            .await
            .expect("create entity");
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                provider.as_ref(),
            )
            .await
            .expect("embed entity");
            candidates.push(record.id);
        }

        let session = initialize(&request).await;
        let found = mcp_call(
            &request,
            &session,
            &read_key,
            2,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "beta", "limit": 2 } }),
        )
        .await;
        assert_eq!(found["result"]["isError"], false, "MCP call: {found}");
        let hits = tool_result_json(&found);
        assert_eq!(
            hits[0]["entity"]["data"]["title"], "beta plan",
            "nearest entity first: {hits}"
        );
        let rest = request
            .get("/api/search?query_text=beta&limit=2")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        rest.assert_status(StatusCode::OK);
        assert_eq!(
            rest.json::<Value>(),
            hits,
            "search results agree across transports"
        );
        let filtered = mcp_call(
            &request,
            &session,
            &read_key,
            5,
            "tools/call",
            json!({ "name": "search_entities", "arguments": {
                "query_text": "beta", "entity_type": "note",
                "filter": { "title": "alpha plan" }, "limit": 1
            } }),
        )
        .await;
        let rest = request
            .get("/api/search")
            .add_query_param("query_text", "beta")
            .add_query_param("entity_type", "note")
            .add_query_param("filter", r#"{"title":"alpha plan"}"#)
            .add_query_param("limit", "1")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
            assert_eq!(filtered["result"]["isError"], true, "{filtered}");
            assert!(
                filtered["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("content filtering is not supported on SQLite")
            );
            rest.assert_status(StatusCode::NOT_IMPLEMENTED);
            assert_eq!(rest.json::<Value>()["error"]["code"], "backend_unsupported");
        } else {
            assert_eq!(filtered["result"]["isError"], false, "{filtered}");
            let filtered_hits = tool_result_json(&filtered);
            assert_eq!(filtered_hits.as_array().unwrap().len(), 1);
            assert_eq!(filtered_hits[0]["entity"]["id"], candidates[0].to_string());
            rest.assert_status(StatusCode::OK);
            assert_eq!(
                rest.json::<Value>(),
                filtered_hits,
                "filters agree across transports"
            );
        }
        let calls = inputs.lock().unwrap().len();
        let (failed_provider, _failed_server) = unavailable_provider().await;
        let reindex =
            entity_embeddings::reindex_workspace(&ctx.db, workspace_id, failed_provider.as_ref())
                .await
                .expect("record failed reindex");
        assert_eq!(reindex.failures.len(), candidates.len());
        let unavailable = mcp_call(
            &request,
            &session,
            &read_key,
            4,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "beta" } }),
        )
        .await;
        assert_eq!(unavailable["result"]["isError"], true, "{unavailable}");
        assert!(
            unavailable["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("reindexed")
        );
        let rest = request
            .get("/api/search?query_text=beta")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(rest.status_code(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(rest.json::<Value>()["error"]["code"], "backend_unavailable");
        assert_eq!(inputs.lock().unwrap().len(), calls);
        let recovered =
            entity_embeddings::reindex_workspace(&ctx.db, workspace_id, provider.as_ref())
                .await
                .expect("recover reindex");
        assert!(recovered.failures.is_empty());
        // The worker's provider is replaced by one that fails: the tool reports the worker's diagnostic.
        ctx.shared_store.insert(failed_provider);
        let failed = mcp_call(
            &request,
            &session,
            &read_key,
            3,
            "tools/call",
            json!({ "name": "search_entities", "arguments": { "query_text": "alpha" } }),
        )
        .await;
        assert_eq!(failed["result"]["isError"], true, "MCP call: {failed}");
        assert!(
            failed["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("HTTP 503"))
        );
        assert!(!failed.to_string().contains("private upstream diagnostic"));
    })
    .await;
}

#[tokio::test]
async fn mcp_membership_tools_treat_tenant_users_as_tenant_bound_accounts() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_admin_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let user_name = format!("mcp-member-{}", Uuid::new_v4());
        let auth = format!("Bearer {tenant_admin_key}");
        let created = request
            .post("/api/members/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let user_id = created.json::<Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let session = initialize(&request).await;

        let listed = mcp_call(
            &request,
            &session,
            &tenant_admin_key,
            2,
            "tools/call",
            json!({"name":"list_members","arguments":{}}),
        )
        .await;
        let member = tool_result_json(&listed)
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_name"] == user_name)
            .cloned()
            .expect("tenant user listed");
        assert!(member["email"].is_null());

        // The tenant user's own key reads its tenant-bound profile.
        let login = request
            .post("/auth/tenant/login")
            .add_header("x-tenant-id", tenant.tenant_id.to_string())
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        login.assert_status_ok();
        let own_key = login.json::<Value>()["api_key"]
            .as_str()
            .unwrap()
            .to_owned();
        let profile = mcp_call(
            &request,
            &session,
            &own_key,
            5,
            "tools/call",
            json!({"name":"get_current_user","arguments":{}}),
        )
        .await;
        let profile = tool_result_json(&profile);
        assert_eq!(profile["user_name"], user_name);
        assert!(profile["email"].is_null());

        // Removing the membership removes the account.
        let removed = mcp_call(
            &request,
            &session,
            &tenant_admin_key,
            6,
            "tools/call",
            json!({"name":"remove_member","arguments":{"user_id":user_id}}),
        )
        .await;
        assert_eq!(tool_result_json(&removed)["removed"], true);
        request
            .post("/auth/tenant/login")
            .add_header("x-tenant-id", tenant.tenant_id.to_string())
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn mcp_tenant_users_cannot_join_another_tenant() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        if crate::requests::skip_unlicensed(&ctx) {
            return;
        }
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user_name = format!("stay-home-{}", Uuid::new_v4());
        let tenant_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id)
                .await;
        let auth = format!("Bearer {tenant_key}");
        let session = initialize(&request).await;
        // A tenant user belongs to its own tenant only.
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let created = request
            .post("/api/members/users")
            .add_header("Authorization", &auth)
            .json(&json!({"user_name":user_name,"password":"tenant-user-password"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let home_user = created.json::<Value>()["user_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let other_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, other.tenant_id, other.owner_id)
                .await;
        let joined = mcp_call(
            &request,
            &session,
            &other_key,
            7,
            "tools/call",
            json!({"name":"remove_member","arguments":{"user_id":home_user}}),
        )
        .await;
        assert_eq!(joined["result"]["isError"], false, "{joined}");
        let listed = mcp_call(
            &request,
            &session,
            &tenant_key,
            8,
            "tools/call",
            json!({"name":"list_members","arguments":{}}),
        )
        .await;
        assert!(
            tool_result_json(&listed)
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["user_id"] == home_user),
            "another tenant cannot remove this tenant's user"
        );
    })
    .await;
}
