use axum::http::StatusCode;
use serde_json::{Value, json};
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::{entity_entities, schema_schemas};

use crate::requests::fixtures::{TenantArgs, TestTenant, issue_api_key};

use super::*;

/// High-dimensional HNSW retains the nearest original cosine through both transports.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one indexed corpus compares concurrent REST and MCP ranking before and after the nearest vector changes"
)]
async fn postgres_search_returns_the_nearest_document_across_code_collapse_and_recovery() {
    use crate::workers::query_embedding::harness::boot_with_query_worker;
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
                            #[expect(clippy::cast_precision_loss, reason = "the remainder is below 104729 and exactly representable as f32")]
                            let coordinate = ((i + 1) * (axis + 11) * 7919 % 104_729) as f32;
                            coordinate / 104_729.0 - 0.5
                        })
                        .collect::<Vec<_>>(),
                ))),
            });
        }
        // Direct write (a): no operation stores a chosen vector, and the decoy corpus must hold exact vectors in bulk.
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
            // Direct write (a): no operation stores a chosen vector, and each phase needs the nearest neighbour's exact coordinates.
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
#[expect(
    clippy::too_many_lines,
    reason = "one worker-backed search scenario compares transports and filtering across reindex failure, recovery, and provider failure"
)]
async fn mcp_search_asks_a_worker_to_embed_the_query() {
    use crate::http_stub::{keyword_provider, unavailable_provider};
    use crate::workers::query_embedding::harness::boot_with_query_worker;
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
