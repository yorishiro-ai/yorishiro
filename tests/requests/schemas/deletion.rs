use axum::http::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set};
use serde_json::json;
use uuid::Uuid;
use yorishiro::{
    App,
    models::_entities::{
        entity_snapshots, inference_proposals, workspace_schema_fork_heads, workspace_schema_forks,
    },
};

use super::create_schema_version;
use crate::requests::{
    boot_request,
    fixtures::{TenantArgs, TestTenant},
};

#[tokio::test]
async fn archived_schema_deletion_names_every_history_reference() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = json!({"name":"history-blockers", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        let mut versions = Vec::new();
        for _ in 0..7 {
            versions.push(create_schema_version(&request, &tenant.api_key, &definition).await.parse::<Uuid>().unwrap());
        }
        entity_snapshots::ActiveModel {
            id: Set(Uuid::now_v7()), job_id: Set(Uuid::now_v7()), workspace_id: Set(tenant.workspace_id),
            entity_id: Set(Uuid::now_v7()), schema_id: Set(versions[1]), schema_version: Set(2), data: Set(json!({"title":"retained"})),
            ..Default::default()
        }.insert(&ctx.db).await.unwrap();
        inference_proposals::ActiveModel {
            id: Set(Uuid::now_v7()), job_id: Set(Uuid::now_v7()), workspace_id: Set(tenant.workspace_id),
            entity_id: Set(Uuid::now_v7()), schema_id: Set(versions[2]), schema_version: Set(3),
            source_field: Set("title".into()), proposed: Set(json!("suggested")), status: Set("pending".into()),
            ..Default::default()
        }.insert(&ctx.db).await.unwrap();
        let fork = workspace_schema_forks::ActiveModel {
            id: Set(Uuid::now_v7()), tenant_id: Set(tenant.tenant_id), workspace_id: Set(tenant.workspace_id),
            source_workspace_id: Set(tenant.workspace_id), source_schema_id: Set(versions[3]),
            source_schema_version: Set(4), source_schema_name: Set("history-blockers".into()),
            fork_schema_id: Set(versions[4]), customized: Set(false), ..Default::default()
        }.insert(&ctx.db).await.unwrap();
        workspace_schema_fork_heads::ActiveModel {
            id: Set(Uuid::now_v7()), tenant_id: Set(tenant.tenant_id), workspace_id: Set(tenant.workspace_id),
            fork_id: Set(fork.id), schema_id: Set(versions[5]), ..Default::default()
        }.insert(&ctx.db).await.unwrap();
        for (version, blocker) in [(0,"workspaces"), (1,"entity snapshots"), (2,"inference proposals"), (3,"schema forks"), (4,"schema forks"), (5,"schema fork heads")] {
            let response = request.delete(&format!("/api/schemas/{}", versions[version]))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
            response.assert_status(StatusCode::CONFLICT);
            assert!(response.text().contains(blocker), "missing {blocker}: {}", response.text());
            request.get(&format!("/api/schemas/{}", versions[version]))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::OK);
        }
    }).await;
}

#[tokio::test]
async fn entity_creation_racing_archival_and_deletion_never_returns_internal_error() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = json!({"name":"delete-race", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        create_schema_version(&request, &tenant.api_key, &definition).await;
        let target = create_schema_version(&request, &tenant.api_key, &definition).await;
        let barrier = tokio::sync::Barrier::new(2);
        let insert = async {
            barrier.wait().await;
            request.post("/api/entities")
                .add_header("Authorization", format!("Bearer {}", tenant.api_key))
                .json(&json!({"schema_name":"delete-race","entity_type":"note","data":{"title":"racer"}})).await
        };
        let delete = async {
            barrier.wait().await;
            create_schema_version(&request, &tenant.api_key, &definition).await;
            request.delete(&format!("/api/schemas/{target}"))
                .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await
        };
        let (inserted, deleted) = tokio::join!(insert, delete);
        inserted.assert_status(StatusCode::CREATED);
        assert!([StatusCode::NO_CONTENT, StatusCode::CONFLICT].contains(&deleted.status_code()), "{}", deleted.text());
        if deleted.status_code() == StatusCode::CONFLICT { assert!(deleted.text().contains("entities")); }
    }).await;
}

#[tokio::test]
async fn schema_deletion_sees_incoming_forks_outside_the_selected_workspace() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::{schema_schemas, tenancy};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let definition = json!({"name":"incoming-fork", "entity_types":{"note":{"fields":{"title":{"type":"string"}}}}});
        create_schema_version(&request, &tenant.api_key, &definition).await;
        let source: Uuid = create_schema_version(&request, &tenant.api_key, &definition).await.parse().unwrap();
        create_schema_version(&request, &tenant.api_key, &definition).await;
        let txn = ctx.db.begin().await.unwrap();
        let destination_name = format!("incoming-fork-target-{}", Uuid::now_v7());
        let destination = tenancy::create_workspace(&txn, tenant.tenant_id, &destination_name, None, None).await.unwrap();
        let target = schema_schemas::create_schema(&txn, tenant.tenant_id, destination.id,
            serde_json::from_value(definition).unwrap(), None, None).await.unwrap();
        workspace_schema_forks::ActiveModel {
            id: Set(Uuid::now_v7()), tenant_id: Set(tenant.tenant_id), workspace_id: Set(destination.id),
            source_workspace_id: Set(tenant.workspace_id), source_schema_id: Set(source),
            source_schema_version: Set(2), source_schema_name: Set("incoming-fork".into()),
            fork_schema_id: Set(target.0.id), customized: Set(false), ..Default::default()
        }.insert(&txn).await.unwrap();
        txn.commit().await.unwrap();
        let response = request.delete(&format!("/api/schemas/{source}"))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        response.assert_status(StatusCode::CONFLICT);
        assert!(response.text().contains("schema forks"), "{}", response.text());
        assert!(!response.text().contains(&destination.id.to_string()));
        let workspace = request.delete(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await;
        workspace.assert_status(StatusCode::CONFLICT);
        assert!(workspace.text().contains("schema forks"), "{}", workspace.text());
        assert!(!workspace.text().contains(&destination.id.to_string()));
        request.get(&format!("/api/workspaces/{}", tenant.workspace_id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key)).await.assert_status(StatusCode::OK);
    }).await;
}
