use crate::controllers::mcp;
use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};
use axum::http::StatusCode;
use sea_orm::EntityTrait;
use yorishiro::App;
use yorishiro::db::DbHandle;
use yorishiro::edition::ee::controllers::middleware::auth::WORKSPACE_HEADER;
use yorishiro::edition::ee::models::access_keys::create_tenant_access_key;

/// Enterprise edition composition must not break authentication on a route base itself defines.
#[tokio::test]
async fn a_workspace_scoped_key_still_works_on_a_base_route() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key, .. } =
            TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .get("/api/schemas")
            .add_header("Authorization", format!("Bearer {api_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            200,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// Tenant keys may omit workspace selection for IAM-filtered lists and search.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one tenant credential and MCP session compare workspace selection, missing headers, and inherited access across requests"
)]
async fn a_tenant_scoped_key_resolves_the_workspace_named_by_the_header() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let user_id = crate::requests::fixtures::create_iam_administrator(&ctx, tenant_id).await;

        // create_tenant_access_key reads tables that yorishiro_app cannot read, so it runs on the identity pool.
        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let identity = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(db.identity.clone());
        let tenant_key = create_tenant_access_key(&identity, tenant_id, Some(user_id))
            .await
            .expect("issuing a tenant-scoped key");

        let created = request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .json(&serde_json::json!({"name":"successor"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let replacement = created.json::<serde_json::Value>();
        assert!(replacement["workspace_id"].is_null());
        assert_eq!(replacement["tenant_id"], tenant_id.to_string());
        request
            .delete(&format!("/api/access-keys/{}", tenant_key.id))
            .add_header("Authorization", format!("Bearer {}", replacement["access_key"].as_str().unwrap()))
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let tenant_key = yorishiro::edition::ee::models::access_keys::CreatedTenantAccessKey {
            id: replacement["id"].as_str().unwrap().parse().unwrap(),
            plaintext: replacement["access_key"].as_str().unwrap().to_owned(),
        };

        // Tenant-level discovery works without selecting a workspace.
        let no_header = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await;
        assert_eq!(
            no_header.status_code(),
            200,
            "response: {:?}",
            no_header.text()
        );

        let with_header = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .await;
        assert_eq!(
            with_header.status_code(),
            200,
            "response: {:?}",
            with_header.text()
        );
        request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .add_header("x-tenant-id", tenant_id.to_string())
            .await
            .assert_status(StatusCode::OK);
        request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .add_header("x-tenant-id", uuid::Uuid::new_v4().to_string())
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .add_header("x-tenant-id", "invalid")
            .await
            .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        let persisted = yorishiro::models::access_keys::Entity::find_by_id(tenant_key.id)
            .one(&identity)
            .await
            .unwrap()
            .unwrap();
        assert!(
            persisted.last_used_at.is_some(),
            "a read-only request must persist last_used_at outside its rolled-back transaction"
        );
        let missing_target = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await;
        assert_eq!(missing_target.status_code(), 200);
        let tenant_identity = missing_target.json::<serde_json::Value>();
        assert_eq!(tenant_identity["workspace_id"], workspace_id.to_string());
        assert!(tenant_identity.get("workspace_access").is_none());
        assert_eq!(tenant_identity["tenant_root"], false);
        request
            .get("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await
            .assert_status(StatusCode::OK);
        request
            .get("/api/search?query_text=test")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await
            .assert_status(StatusCode::SERVICE_UNAVAILABLE);
        request
            .get("/api/tenants/current")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .await
            .assert_status(StatusCode::OK);
        let response = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, workspace_id.to_string())
            .await;
        assert_eq!(response.status_code(), 200, "{}", response.text());
        let identity = response.json::<serde_json::Value>();
        assert_eq!(identity["workspace_id"], workspace_id.to_string());
        assert_eq!(identity["tenant_id"], tenant_id.to_string());
        assert_eq!(identity["user_id"], user_id.to_string());
        assert_eq!(identity["identity_kind"], "user");
        assert_ne!(identity["workspace_name"].as_str().unwrap(), "");
        assert_ne!(identity["tenant_name"].as_str().unwrap(), "");
        assert!(!response.text().contains(&tenant_key.plaintext));
        assert!(!response.text().contains("key_hash"));
        // Each MCP call resolves its own headers, including calls in the same session.
        let session = mcp::initialize(&request).await;
        for (id, with_header) in [(2u64, true), (3, false)] {
            let mut call = request
                .post("/mcp")
                .add_header("Accept", "application/json, text/event-stream")
                .add_header("MCP-Session-Id", session.clone())
                .add_header("MCP-Protocol-Version", "2025-03-26")
                .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext));
            if with_header {
                call = call.add_header(WORKSPACE_HEADER, workspace_id.to_string());
            }
            let response = call
                .json(&serde_json::json!({
                    "jsonrpc":"2.0", "id":id, "method":"tools/call",
                    "params":{"name":"get_caller_identity","arguments":{}}
                }))
                .await;
            assert_eq!(response.status_code(), 200, "{}", response.text());
            let body = mcp::rpc_body(&response.text());
            assert_ne!(body["result"]["isError"], true, "{body}");
            let mcp: serde_json::Value =
                serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                mcp,
                if with_header {
                    &identity
                } else {
                    &tenant_identity
                }
                .clone()
            );
            assert!(!response.text().contains(&tenant_key.plaintext));
            assert!(!response.text().contains("key_hash"));
        }
        let response = request
            .post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .add_header("MCP-Session-Id", session.clone())
            .add_header("MCP-Protocol-Version", "2025-03-26")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .json(&serde_json::json!({"jsonrpc":"2.0", "id":4, "method":"tools/call", "params":{"name":"list_schemas","arguments":{}}}))
            .await;
        let body = mcp::rpc_body(&response.text());
        assert_eq!(body["result"]["isError"], false, "{body}");
        let response = request
            .post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .add_header("MCP-Session-Id", session)
            .add_header("MCP-Protocol-Version", "2025-03-26")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .json(&serde_json::json!({"jsonrpc":"2.0", "id":5, "method":"tools/call", "params":{"name":"search_entities","arguments":{"query_text":"test"}}}))
            .await;
        let body = mcp::rpc_body(&response.text());
        assert_eq!(body["result"]["isError"], true, "{body}");
        assert!(!body.to_string().contains("x-workspace-id"), "{body}");
    })
    .await;
}

#[tokio::test]
async fn tenant_key_rechecks_iam_policy_after_detachment() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::{access_keys, tenancy};

    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.unwrap();
        let member = tenancy::register_member(&txn, tenant.tenant_id,
            &crate::requests::fixtures::unique_email("tenant-target"), "hunter2-hunter2", None).await.unwrap();
        txn.commit().await.unwrap();
        let key = create_tenant_access_key(&ctx.db, tenant.tenant_id, Some(member.id)).await.unwrap();
        let root_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let created = request.post("/api/policies")
            .add_header("Authorization", format!("Bearer {root_key}"))
            .json(&serde_json::json!({"name":"schema-reader", "document":{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["schema:ListSchemas"],"Resource":[format!("yrn:workspace:{}:{}", tenant.tenant_id, tenant.workspace_id)]}]}})).await;
        created.assert_status(StatusCode::CREATED);
        let policy_id = created.json::<serde_json::Value>()["policy_id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/users/{}/policies/{policy_id}", member.id);
        request.put(&attachment).add_header("Authorization", format!("Bearer {root_key}"))
            .await.assert_status(StatusCode::NO_CONTENT);
        let session = mcp::initialize(&request).await;
        for (id, allowed) in [(2, true), (3, false), (4, true)] {
            if id == 3 {
                request.delete(&attachment).add_header("Authorization", format!("Bearer {root_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            } else if id == 4 {
                request.put(&attachment).add_header("Authorization", format!("Bearer {root_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
            let response = request.get("/api/schemas")
                .add_header("Authorization", format!("Bearer {}", key.plaintext))
                .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string()).await;
            assert_eq!(response.status_code().as_u16(), if allowed { 200 } else { 404 }, "{}", response.text());
            let response = request.post("/mcp")
                .add_header("Accept", "application/json, text/event-stream")
                .add_header("MCP-Session-Id", session.clone())
                .add_header("MCP-Protocol-Version", "2025-03-26")
                .add_header("Authorization", format!("Bearer {}", key.plaintext))
                .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
                .json(&serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"list_schemas","arguments":{}}})).await;
            let body = mcp::rpc_body(&response.text());
            if allowed {
                assert_ne!(body["result"]["isError"], true, "{body}");
            } else {
                assert_eq!(body["result"]["isError"], true, "{body}");
                assert_eq!(body["result"]["structuredContent"]["error"]["code"], "not_found");
            }
            assert!(access_keys::Entity::find_by_id(key.id).one(&ctx.db).await.unwrap().is_some());
        }
    }).await;
}

/// A key cannot select another tenant's workspace through the header, even though the key itself is valid.
#[tokio::test]
async fn a_tenant_scoped_key_cannot_retarget_another_tenants_workspace() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_multitenant_request(|request, ctx| async move {
        let caller = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let foreign = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let membership = yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            foreign.tenant_id,
            caller.owner_id,
        )
        .await;
        assert!(matches!(
            membership,
            Err(yorishiro::YorishiroError::Conflict { .. })
        ));

        let db = ctx.shared_store.get::<DbHandle>().unwrap();
        let identity = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(db.identity.clone());
        let tenant_key =
            create_tenant_access_key(&identity, caller.tenant_id, Some(caller.owner_id))
                .await
                .expect("issuing a tenant-scoped key");

        // The key works in its own tenant's workspace.
        let own = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, caller.workspace_id.to_string())
            .await;
        assert_eq!(own.status_code(), 200, "{}", own.text());

        // A foreign workspace gives the same response as a missing workspace.
        let foreign_target = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, foreign.workspace_id.to_string())
            .await;
        foreign_target.assert_status(StatusCode::NOT_FOUND);
        let missing_target = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, uuid::Uuid::now_v7().to_string())
            .await;
        missing_target.assert_status(StatusCode::NOT_FOUND);
        assert_eq!(
            foreign_target.json::<serde_json::Value>(),
            missing_target.json::<serde_json::Value>()
        );

        // The same header is valid for a key of the foreign tenant, proving the refusal is about the pairing, not the workspace.
        let foreign_key =
            create_tenant_access_key(&identity, foreign.tenant_id, Some(foreign.owner_id))
                .await
                .expect("issuing a second tenant-scoped key");
        let valid = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", foreign_key.plaintext))
            .add_header(WORKSPACE_HEADER, foreign.workspace_id.to_string())
            .await;
        assert_eq!(valid.status_code(), 200, "{}", valid.text());
    })
    .await;
}
