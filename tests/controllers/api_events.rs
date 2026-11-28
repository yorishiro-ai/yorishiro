//! Every REST request and MCP call leaves rows in the durable, append-only trail.

use axum::http::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use yorishiro::models::api_events::{Entity, Model};

use crate::requests::boot_request;
use crate::requests::fixtures::{TenantArgs, TestTenant};

async fn events_of(ctx: &loco_rs::app::AppContext, request_id: &str) -> Vec<Model> {
    Entity::find()
        .filter(yorishiro::models::_entities::api_events::Column::RequestId.eq(request_id))
        .order_by_asc(yorishiro::models::_entities::api_events::Column::Id)
        .all(&ctx.db)
        .await
        .expect("read the trail")
}

async fn root_key(ctx: &loco_rs::app::AppContext, tenant: &TestTenant) -> String {
    crate::requests::fixtures::issue_tenant_key(ctx, tenant.tenant_id, tenant.owner_id).await
}

#[tokio::test]
async fn a_read_and_a_write_record_who_when_where_what_and_the_outcome() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let bearer = format!("Bearer {}", root_key(&ctx, &tenant).await);

        request
            .get("/api/workspaces")
            .add_header("Authorization", &bearer)
            .add_header("x-request-id", "trail-read")
            .add_header("user-agent", "trail-agent/1")
            .await
            .assert_status_ok();
        let read = events_of(&ctx, "trail-read").await;
        assert_eq!(read.len(), 1, "a read leaves one completed row");
        let row = &read[0];
        assert_eq!(row.phase, "completed");
        assert_eq!(row.transport, "rest");
        assert_eq!(row.event_name, "GET /api/workspaces");
        assert!(row.read_only);
        assert_eq!(row.user_agent, "trail-agent/1");
        assert_eq!(row.outcome.as_deref(), Some("success"));
        assert_eq!(row.status_code, Some(200));
        assert_eq!(row.tenant_id, Some(tenant.tenant_id));
        assert!(row.user_id.is_some());
        assert!(row.access_key_id.is_some());
        assert!(row.caller.as_deref().unwrap().starts_with("yrn:iam:"));
        assert!(row.latency_ms.is_some());

        let created = request
            .post("/api/workspaces")
            .add_header("Authorization", &bearer)
            .add_header("x-request-id", "trail-write")
            .json(&json!({"name": "trail-workspace"}))
            .await;
        let write = events_of(&ctx, "trail-write").await;
        let phases: Vec<_> = write.iter().map(|row| row.phase.as_str()).collect();
        assert_eq!(
            phases,
            ["started", "completed"],
            "a write is recorded before it runs, once it has an identity, and when it ends"
        );
        assert_eq!(write[0].tenant_id, Some(tenant.tenant_id));
        assert_eq!(write[1].tenant_id, Some(tenant.tenant_id));
        assert_eq!(write[0].caller, write[1].caller);
        assert!(write.iter().all(|row| !row.read_only));
        assert!(write[..1].iter().all(|row| row.outcome.is_none()));
        assert_eq!(
            write[1].status_code,
            Some(i32::from(created.status_code().as_u16()))
        );
        assert!(
            write[1]
                .permissions
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check
                    .as_str()
                    .unwrap()
                    .starts_with("workspace:CreateWorkspace "))
        );
    })
    .await;
}

#[tokio::test]
async fn unauthenticated_and_denied_calls_are_recorded_without_secrets() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        request
            .get("/api/workspaces")
            .add_header("x-request-id", "trail-anonymous")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        request
            .get("/api/workspaces?secret=query-sentinel")
            .add_header("Authorization", "Bearer ysr_sentinel-presented-key")
            .add_header("x-request-id", "trail-bad-key")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        for id in ["trail-anonymous", "trail-bad-key"] {
            let rows = events_of(&ctx, id).await;
            assert_eq!(rows.len(), 1, "{id}");
            assert_eq!(rows[0].outcome.as_deref(), Some("denied"));
            assert_eq!(rows[0].tenant_id, None);
            assert_eq!(rows[0].caller, None);
        }
        let everything =
            serde_json::to_string(&Entity::find().all(&ctx.db).await.unwrap()).unwrap();
        assert!(!everything.contains("sentinel"));
    })
    .await;
}

#[tokio::test]
async fn mcp_calls_leave_the_envelope_and_the_tool_call() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = root_key(&ctx, &tenant).await;
        let session = super::mcp::initialize(&request).await;
        let called = request
            .post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .add_header("MCP-Session-Id", session)
            .add_header("MCP-Protocol-Version", "2025-03-26")
            .add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-request-id", "trail-mcp")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_workspaces","arguments":{}}}))
            .await;
        called.assert_status_ok();
        let rows = events_of(&ctx, "trail-mcp").await;
        let tool: Vec<_> = rows.iter().filter(|row| row.transport == "mcp").collect();
        assert_eq!(tool.len(), 1);
        assert_eq!(tool[0].event_name, "list_workspaces");
        assert!(tool[0].read_only);
        assert_eq!(tool[0].outcome.as_deref(), Some("success"));
        assert_eq!(tool[0].tenant_id, Some(tenant.tenant_id));
        assert!(rows.iter().any(|row| row.transport == "mcp_http" && row.event_name == "POST /mcp"));
        let listed = request
            .post("/mcp")
            .add_header("Accept", "application/json, text/event-stream")
            .add_header("MCP-Session-Id", super::mcp::initialize(&request).await)
            .add_header("MCP-Protocol-Version", "2025-03-26")
            .add_header("x-request-id", "trail-mcp-list")
            .json(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}))
            .await;
        listed.assert_status_ok();
        assert!(events_of(&ctx, "trail-mcp-list").await.iter().any(|row| {
            row.transport == "mcp" && row.event_name == "tools/list" && row.read_only
        }));
    })
    .await;
}

#[cfg(feature = "enterprise")]
#[tokio::test]
async fn the_trail_is_listed_by_policy_and_only_for_the_callers_tenant() {
    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_multitenant_request(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator =
            crate::requests::fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator)
                .await;
        request
            .get("/api/workspaces")
            .add_header(
                "Authorization",
                format!("Bearer {}", root_key(&ctx, &other).await),
            )
            .add_header("x-request-id", "trail-other-tenant")
            .await
            .assert_status_ok();
        request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .add_header("x-request-id", "trail-own-tenant")
            .await
            .assert_status_ok();

        let listed = request
            .get("/api/api-events?limit=200")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        listed.assert_status_ok();
        let events: Vec<Value> = listed.json();
        assert!(
            events
                .iter()
                .any(|event| event["request_id"] == "trail-own-tenant")
        );
        assert!(
            events
                .iter()
                .all(|event| event["request_id"] != "trail-other-tenant")
        );
        assert!(
            events
                .iter()
                .all(|event| event["tenant_id"] == tenant.tenant_id.to_string())
        );
        let one: Vec<Value> = request
            .get("/api/api-events?request_id=trail-own-tenant")
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .json();
        assert!(
            !one.is_empty()
                && one
                    .iter()
                    .all(|event| event["request_id"] == "trail-own-tenant")
        );

        // ReadOnlyAccess includes the audit listings, as it does in AWS; a member with no policy has none.
        let reader = crate::requests::fixtures::create_iam_reader(&ctx, tenant.tenant_id).await;
        let reader_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, reader).await;
        request
            .get("/api/api-events")
            .add_header("Authorization", format!("Bearer {reader_key}"))
            .await
            .assert_status_ok();
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let bare = yorishiro::models::tenancy::register_member(
            &txn,
            tenant.tenant_id,
            "no-policy@example.org",
            "hunter2-hunter2",
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let bare_key =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, bare.id).await;
        request
            .get("/api/api-events")
            .add_header("Authorization", format!("Bearer {bare_key}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn a_background_job_is_followed_from_its_request_to_its_end() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let accepted = request
            .post("/api/import-jobs")
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .add_header("x-request-id", "trail-job")
            .text("")
            .await;
        accepted.assert_status(StatusCode::ACCEPTED);
        let id = accepted.json::<Value>()["import_job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        crate::requests::jobs::finish_via_api(
            &request,
            &ctx,
            id,
            &tenant.api_key,
            &format!("/api/import-jobs/{id}"),
        )
        .await;
        let rows = events_of(&ctx, "trail-job").await;
        let job: Vec<_> = rows
            .iter()
            .filter(|row| row.transport == "job" && row.phase == "completed")
            .collect();
        assert_eq!(
            job.len(),
            1,
            "the request's trail ends with the job: {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.transport == "job"
                && row.phase == "started"
                && row.attempt == Some(1)),
            "the attempt that ran the job is in the trail: {rows:#?}"
        );
        assert_eq!(job[0].job_id, Some(id));
        assert_eq!(job[0].event_name, "import");
        assert_eq!(job[0].tenant_id, Some(tenant.tenant_id));
        assert_eq!(job[0].workspace_id, Some(tenant.workspace_id));
        assert_eq!(job[0].outcome.as_deref(), Some("success"));
        assert!(
            rows.iter()
                .any(|row| row.transport == "rest" && row.phase == "completed")
        );
    })
    .await;
}

#[tokio::test]
async fn the_trail_can_be_filtered_by_a_resource_and_a_correlation() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let bearer = format!("Bearer {}", root_key(&ctx, &tenant).await);
        let group = request
            .post("/api/groups")
            .add_header("Authorization", &bearer)
            .add_header("x-request-id", "trail-group")
            .json(&json!({"name": "filtered"}))
            .await;
        group.assert_status(StatusCode::CREATED);
        let group_id = group.json::<Value>()["group_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let by_resource = request
            .get(&format!("/api/api-events?resource={group_id}"))
            .add_header("Authorization", &bearer)
            .await;
        by_resource.assert_status_ok();
        let rows = by_resource.json::<Vec<Value>>();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["request_id"], "trail-group");
        assert_eq!(rows[0]["correlation_id"], "trail-group");
        let other = request
            .get(&format!(
                "/api/api-events?resource={}",
                uuid::Uuid::new_v4()
            ))
            .add_header("Authorization", &bearer)
            .await;
        assert_eq!(other.json::<Vec<Value>>(), Vec::<Value>::new());
        let by_correlation = request
            .get("/api/api-events?correlation_id=trail-group")
            .add_header("Authorization", &bearer)
            .await;
        assert_ne!(by_correlation.json::<Vec<Value>>(), Vec::<Value>::new());
    })
    .await;
}

#[tokio::test]
async fn the_trail_keeps_identifiers_and_paging_but_never_content() {
    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let bearer = format!("Bearer {}", root_key(&ctx, &tenant).await);
        request
            .get(&format!(
                "/api/workspaces/{}?limit=5&name=secret-name&entity_id={}",
                tenant.workspace_id,
                uuid::Uuid::nil()
            ))
            .add_header("Authorization", &bearer)
            .add_header("x-request-id", "trail-params")
            .await;
        let rest = &events_of(&ctx, "trail-params").await[0];
        assert_eq!(
            rest.request_parameters,
            json!({"limit": 5, "entity_id": uuid::Uuid::nil().to_string(), "path": [tenant.workspace_id.to_string()]})
        );

        let session = crate::controllers::mcp::initialize(&request).await;
        crate::controllers::mcp::call_tool(
            &request,
            &session,
            &root_key(&ctx, &tenant).await,
            "search_entities",
            json!({"query_text": "private words", "workspace_id": tenant.workspace_id, "limit": 3}),
        )
        .await;
        let rows = Entity::find()
            .filter(yorishiro::models::_entities::api_events::Column::EventName.eq("search_entities"))
            .all(&ctx.db)
            .await
            .unwrap();
        let params = &rows[0].request_parameters;
        assert_eq!(params["workspace_id"], tenant.workspace_id.to_string());
        assert_eq!(params["limit"], 3);
        assert!(!params.to_string().contains("private words"), "{params}");
    })
    .await;
}
