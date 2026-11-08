use crate::requests::boot_request;
#[cfg(feature = "enterprise")]
use sea_orm::ConnectionTrait;
use sea_orm::sea_query::ExprTrait;
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QuerySelect, SelectExt,
    Statement,
};
use yorishiro::App;
use yorishiro::error::YorishiroError;
use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
use yorishiro::models::entity_embeddings;
use yorishiro::models::{entity_entities, schema_schemas, search};
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

fn note_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "note",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true, "x-embed": true } } }
        }
    })
}

#[tokio::test]
async fn postgres_rrf_promotes_rare_text_and_gates_common_text() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(&ctx.db, fixture.tenant_id, fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(), None, None).await.unwrap();
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};
        use yorishiro::models::_entities::entity_embeddings as vectors;
        let mut rare = None;
        for i in 0..60 {
            let entity = entity_entities::create(&ctx.db, fixture.workspace_id, entity_entities::CreateEntityInput {
                schema_name:"note".into(), entity_type:"note".into(),
                data:serde_json::json!({"title":if i==20 {"common ERR-4821x"} else {"common document"}})
            }, None).await.unwrap();
            vectors::ActiveModel { workspace_id:Set(fixture.workspace_id), entity_id:Set(entity.id),
                tenant_id:Set(fixture.tenant_id), embedding:Set(Some(sea_orm::entity::prelude::PgVector::from(vec![1.0, i as f32 / 60.0 + 0.01]))) }
                .insert(&ctx.db).await.unwrap();
            if i==20 { rare=Some(entity.id); }
        }
        let query = || search::SearchQuery { limit:10, ..Default::default() };
        for (text, eligible) in [("boundary", true), ("ERR-4821x", true), ("café", true), ("Łódź", true), ("Việt", true), ("a\u{301}bc", true), ("éà", false), ("a\u{301}b", false), ("Αθήνα", false), ("Москва", false), ("ab", false), ("東京", false), ("東京都", false)] {
            let statement = ctx.db.get_database_backend().build(&search::postgres_rrf_query(
                fixture.workspace_id, vec![1.0,0.0], text, &query()).unwrap());
            assert_eq!(statement.sql.contains("word_similarity"), eligible, "{text}: {}", statement.sql);
        }
        let mut signed = vec![1.0_f32;4000]; signed[0] = -1.0;
        let binary = search::vector_candidates_query(sea_orm::DatabaseBackend::Postgres, fixture.workspace_id, signed, &query()).unwrap();
        let sql = ctx.db.get_database_backend().build(&binary).sql;
        assert!(sql.contains("<=>") && !sql.contains("halfvec"));
        assert!(!sql.contains("binary_quantize"));
        assert!(!sql.contains("\"w\".\"id\" = \"e\".\"workspace_id\""), "reindex guard must not correlate the ANN join");
        let positive = search::vector_candidates_query(sea_orm::DatabaseBackend::Postgres, fixture.workspace_id, vec![1.0;4096], &query()).unwrap();
        assert!(!ctx.db.get_database_backend().build(&positive).sql.contains("<~>"), "above 4000 dimensions always uses original cosine");
        let mut signed = vec![1.0_f32;4001]; signed[0] = -1.0;
        let exact = search::vector_candidates_query(sea_orm::DatabaseBackend::Postgres, fixture.workspace_id, signed, &query()).unwrap();
        assert!(!ctx.db.get_database_backend().build(&exact).sql.contains("binary_quantize"));
        assert!(search::vector_candidates_query(sea_orm::DatabaseBackend::Postgres, fixture.workspace_id, vec![1.0;4097], &query()).is_err());
        let baseline = search::search_by_vector(&ctx.db, fixture.workspace_id, vec![1.0,0.0], "", query()).await.unwrap();
        let common = search::search_by_vector(&ctx.db, fixture.workspace_id, vec![1.0,0.0], "common", query()).await.unwrap();
        assert_eq!(baseline.iter().map(|h|h.entity.id).collect::<Vec<_>>(),common.iter().map(|h|h.entity.id).collect::<Vec<_>>(), "21+ matches must disable text list");
        let rare_hits = search::search_by_vector(&ctx.db, fixture.workspace_id, vec![1.0,0.0], "ERR-4821x", query()).await.unwrap();
        assert_eq!(rare_hits[0].entity.id, rare.unwrap(), "vector rank21 plus text rank1 must lead RRF");
        assert_eq!(rare_hits.len(),10);
        assert_eq!(rare_hits.iter().map(|h|h.entity.id).collect::<std::collections::HashSet<_>>().len(),10);
        let text_fixture = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(&ctx.db, text_fixture.tenant_id, text_fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(), None, None).await.unwrap();
        let mut first = None;
        for i in 0..21 {
            if i == 20 {
                let hits = search::search_by_vector(&ctx.db, text_fixture.workspace_id, vec![1.0,0.0], "境界 boundary", query()).await.unwrap();
                assert_eq!(hits.len(), 10, "exactly 20 matches must enable text-only results");
                assert_eq!(hits[0].entity.id, first.unwrap(), "closest literal text must rank first");
                assert!(hits.iter().all(|hit| hit.distance.is_none()));
            }
            let title = if i == 0 { "境界 boundary".to_string() } else { format!("境界 boundary {}", "unrelated filler ".repeat(20)) };
            let entity = entity_entities::create(&ctx.db, text_fixture.workspace_id, entity_entities::CreateEntityInput {
                schema_name:"note".into(), entity_type:"note".into(), data:serde_json::json!({"title":title})
            }, None).await.unwrap();
            if i == 0 { first = Some(entity.id); }
        }
        let gated = search::search_by_vector(&ctx.db, text_fixture.workspace_id, vec![1.0,0.0], "境界 boundary", query()).await.unwrap();
        assert!(gated.is_empty(), "exactly 21 text-only matches must disable the text list");
    }).await;
}

#[tokio::test]
async fn postgres_qwen06_materialized_recall() {
    postgres_genuine_recall(
        1024,
        include_str!("../fixtures/search/qwen06.json"),
        include_bytes!("../fixtures/search/qwen06.f32"),
    )
    .await;
}

#[tokio::test]
async fn postgres_qwen8b_hnsw_recall() {
    postgres_genuine_recall(
        4096,
        include_str!("../fixtures/search/qwen8b.json"),
        include_bytes!("../fixtures/search/qwen8b.f32"),
    )
    .await;
}

async fn postgres_genuine_recall(dimensions: usize, metadata: &str, bytes: &[u8]) {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, IntoActiveModel};
    use sha2::{Digest, Sha256};
    use yorishiro::models::_entities::{entity_embeddings as vectors, entity_entities as entities};

    let metadata: serde_json::Value = serde_json::from_str(metadata).unwrap();
    assert_eq!(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        metadata["sha256"].as_str().unwrap()
    );
    assert_eq!(bytes.len(), (2100 + 12) * dimensions * 4);
    let corpus: Vec<Vec<f32>> = bytes
        .chunks_exact(dimensions * 4)
        .map(|row| {
            row.as_chunks::<4>()
                .0
                .iter()
                .map(|value| f32::from_le_bytes(*value))
                .collect()
        })
        .collect();
    assert!(corpus.iter().flatten().all(|value| value.is_finite()));
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let seed = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"corpus seed"}),
            },
            None,
        )
        .await
        .unwrap();
        let original = entities::Entity::find_by_id(seed.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let ids: Vec<_> = (0..2100).map(|_| uuid::Uuid::now_v7()).collect();
        entities::Entity::insert_many(ids.iter().enumerate().map(|(i, id)| {
            let mut row = original.clone().into_active_model();
            row.id = Set(*id);
            row.data = Set(serde_json::json!({"title": metadata["documents"][i]}));
            row
        }))
        .exec(&ctx.db)
        .await
        .unwrap();
        for (id_batch, vector_batch) in ids.chunks(100).zip(corpus[..2100].chunks(100)) {
            vectors::Entity::insert_many(id_batch.iter().zip(vector_batch).map(|(id, vector)| {
                vectors::ActiveModel {
                    workspace_id: Set(fixture.workspace_id),
                    entity_id: Set(*id),
                    tenant_id: Set(fixture.tenant_id),
                    embedding: Set(Some(sea_orm::entity::prelude::PgVector::from(
                        vector.clone(),
                    ))),
                }
            }))
            .exec(&ctx.db)
            .await
            .unwrap();
        }
        let mut workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        workspace.embedding_dimensions = Set(Some(dimensions as i32));
        workspace.embedding_model = Set(Some(metadata["model"].as_str().unwrap().into()));
        workspace.update(&ctx.db).await.unwrap();
        let index = format!("ysr_hnsw_{}_d{dimensions}", fixture.workspace_id.simple());
        let build = 0;
            entity_embeddings::maintain_workspace_indexes(&ctx.db)
                .await
                .unwrap();
            ctx.db
                .execute_unprepared("ANALYZE entity_entities; ANALYZE entity_embeddings")
                .await
                .unwrap();
            let txn = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id)
                .await
                .unwrap();
            for (i, vector) in corpus[2100..].iter().enumerate() {
                let query = search::SearchQuery {
                    limit: 10,
                    ..Default::default()
                };
                let statement = ctx.db.get_database_backend().build(
                    &search::postgres_rrf_query(fixture.workspace_id, vector.clone(), "", &query)
                        .unwrap(),
                );
                if dimensions <= 2000 {
                assert!(
                    statement.sql.contains("MATERIALIZED") && statement.to_string().contains("LIMIT 1000")
                );
                txn.execute_unprepared(
                    "SET LOCAL hnsw.ef_search=400; SET LOCAL hnsw.iterative_scan=relaxed_order",
                )
                .await
                .unwrap();
                let mut explain = statement.clone();
                explain.sql = format!("EXPLAIN (ANALYZE, FORMAT JSON) {}", explain.sql);
                let plan = txn
                    .query_one_raw(explain)
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<serde_json::Value>("", "QUERY PLAN")
                    .unwrap();
                let mut nodes = vec![&plan[0]["Plan"]];
                let mut hnsw_used = false;
                let mut candidates_used = false;
                while let Some(node) = nodes.pop() {
                    if node["Index Name"].as_str() == Some(&index) {
                        assert!(node["Actual Loops"].as_f64().unwrap() > 0.0);
                        assert!(
                            node["Actual Rows"].as_f64().unwrap()
                                * node["Actual Loops"].as_f64().unwrap()
                                <= 1000.0
                        );
                        hnsw_used = true;
                    }
                    if node["CTE Name"].as_str() == Some("ann_candidates") {
                        candidates_used |= node["Actual Loops"].as_f64().unwrap() > 0.0;
                    }
                    if let Some(children) = node["Plans"].as_array() {
                        nodes.extend(children);
                    }
                }
                assert!(hnsw_used && candidates_used, "{plan}");
                } else {
                    let state = txn.query_one_raw(sea_orm::Statement::from_sql_and_values(
                        sea_orm::DatabaseBackend::Postgres,
                        "SELECT dimensions, octet_length(graph) AS bytes, pending FROM entity_embedding_indexes WHERE workspace_id=$1",
                        [fixture.workspace_id.into()],
                    )).await.unwrap().unwrap();
                    assert_eq!(state.try_get::<i32>("", "dimensions").unwrap(), 4096);
                    assert!(state.try_get::<i32>("", "bytes").unwrap() > 0);
                    assert_eq!(state.try_get::<serde_json::Value>("", "pending").unwrap(), serde_json::json!({}));
                }
                let started = std::time::Instant::now();
                let hits =
                    search::search_by_vector(&txn, fixture.workspace_id, vector.clone(), "", query)
                        .await
                        .unwrap();
                println!("genuine dimensions={dimensions} build={build} query={i} search_ms={:.3}", started.elapsed().as_secs_f64()*1000.0);
                txn.execute_unprepared(
                    "SET LOCAL enable_indexscan=off; SET LOCAL enable_bitmapscan=off",
                )
                .await
                .unwrap();
                let exact_statement = ctx.db.get_database_backend().build(
                    &search::vector_candidates_query(
                        sea_orm::DatabaseBackend::Postgres,
                        fixture.workspace_id,
                        vector.clone(),
                        &search::SearchQuery {
                            limit: 10,
                            ..Default::default()
                        },
                    )
                    .unwrap(),
                );
                let exact: Vec<uuid::Uuid> = txn
                    .query_all_raw(exact_statement)
                    .await
                    .unwrap()
                    .iter()
                    .map(|row| row.try_get("", "id").unwrap())
                    .collect();
                txn.execute_unprepared(
                    "SET LOCAL enable_indexscan=on; SET LOCAL enable_bitmapscan=on",
                )
                .await
                .unwrap();
                assert_eq!(hits.len(), 10);
                assert_eq!(hits[0].entity.id, exact[0], "build={build}, query={i}");
                assert!(
                    hits.iter().all(|hit| exact.contains(&hit.entity.id)),
                    "recall@10 must be 1: build={build}, query={i}"
                );
            }
            println!("genuine dimensions={dimensions} build={build}: 12/12 queries recall@10=1 and exact top1; HNSW candidates <=1000");
            txn.rollback().await.unwrap();

    })
    .await;
}

#[tokio::test]
async fn postgres_workspace_hnsw_promotion_and_cleanup() {
    if !crate::require_postgres_backend() {
        return;
    }
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, IntoActiveModel};
    use yorishiro::models::_entities::{entity_embeddings as vectors, entity_entities as entities};
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(&ctx.db, fixture.tenant_id, fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(), None, None).await.unwrap();
        let entity = entity_entities::create(&ctx.db, fixture.workspace_id, entity_entities::CreateEntityInput {
            schema_name:"note".into(), entity_type:"note".into(), data:serde_json::json!({"title":"index fixture"})
        }, None).await.unwrap();
        let original = entities::Entity::find_by_id(entity.id).one(&ctx.db).await.unwrap().unwrap();
        let ids: Vec<_> = (0..2000).map(|_| uuid::Uuid::now_v7()).collect();
        let rows = ids.iter().map(|id| { let mut row = original.clone().into_active_model(); row.id=Set(*id); row });
        entities::Entity::insert_many(rows).exec(&ctx.db).await.unwrap();
        let rows = ids.iter().enumerate().map(|(i, id)| vectors::ActiveModel {
            workspace_id:Set(fixture.workspace_id), entity_id:Set(*id), tenant_id:Set(fixture.tenant_id),
            embedding:Set(Some(sea_orm::entity::prelude::PgVector::from((0..7).map(|axis| {
                (((i + 1) * (axis + 11) * 7919 % 104729) as f32 / 104729.0) + 0.01
            }).collect::<Vec<_>>())))
        });
        vectors::Entity::insert_many(rows).exec(&ctx.db).await.unwrap();
        let mut workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id).one(&ctx.db).await.unwrap().unwrap().into_active_model();
        workspace.embedding_dimensions=Set(Some(7)); workspace.embedding_model=Set(Some("fixture".into()));
        workspace.update(&ctx.db).await.unwrap();
        let index = format!("ysr_hnsw_{}_d7", fixture.workspace_id.simple());
        // A second maintainer must skip while the real session lock is held.
        use sea_orm::sqlx::Connection as _;
        let mut lock = ctx.db.get_postgres_connection_pool().acquire().await.unwrap().detach();
        sea_orm::sqlx::query("SELECT pg_advisory_lock(hashtextextended('embedding-index-maintenance',0))")
            .execute(&mut lock).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), entity_embeddings::maintain_workspace_indexes(&ctx.db)).await.unwrap().unwrap();
        let present: Option<String> = sea_orm::sqlx::query_scalar("SELECT to_regclass($1)::text").bind(&index).fetch_one(&mut lock).await.unwrap();
        assert!(present.is_none(), "a competing maintenance session must not create the index");
        lock.close().await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        let exists = |name: String| sea_orm::ConnectionTrait::query_one_raw(&ctx.db, Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres, "SELECT indisvalid FROM pg_index WHERE indexrelid=to_regclass($1)", [name.into()]));
        assert!(exists(index.clone()).await.unwrap().unwrap().try_get::<bool>("","indisvalid").unwrap());
        // Execute and EXPLAIN the same production candidate builder without forcing an ANN plan.
        use sea_orm::{ConnectionTrait, TransactionTrait};
        let txn = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let role = txn.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            "SELECT current_user AS role, rolsuper FROM pg_roles WHERE rolname=current_user")).await.unwrap().unwrap();
        assert_eq!(role.try_get::<String>("", "role").unwrap(), "yorishiro_app");
        assert!(!role.try_get::<bool>("", "rolsuper").unwrap());
        txn.execute_unprepared("SET LOCAL plan_cache_mode=force_custom_plan; SET LOCAL hnsw.ef_search=200; SET LOCAL hnsw.iterative_scan=relaxed_order").await.unwrap();
        let query = search::SearchQuery { limit: 200, ..Default::default() };
        let vector = vec![1.0_f32;7];
        let statement = ctx.db.get_database_backend().build(&search::postgres_rrf_query(
            fixture.workspace_id, vector.clone(), "", &query).unwrap());
        let mut explain = statement.clone();
        explain.sql = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {}", explain.sql);
        let plan = txn.query_one_raw(explain).await.unwrap().unwrap().try_get::<serde_json::Value>("", "QUERY PLAN").unwrap();
        println!("production vector plan: {plan}");
        assert!(plan.to_string().contains(&index), "production query must use workspace HNSW: {plan}");
        let natural = txn.query_all_raw(statement.clone()).await.unwrap();
        assert_eq!(natural.len(), 200, "candidate limit must not be capped at pgvector's default 40");
        txn.execute_unprepared("SET LOCAL hnsw.ef_search=40; SET LOCAL hnsw.iterative_scan=off").await.unwrap();
        let hits = search::search_by_vector(&txn, fixture.workspace_id, vector, "", query).await.unwrap();
        assert_eq!(hits.len(), 200);
        let settings = txn.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            "SELECT current_setting('hnsw.ef_search') AS ef, current_setting('hnsw.iterative_scan') AS scan")).await.unwrap().unwrap();
        assert_eq!(settings.try_get::<String>("", "ef").unwrap(), "40");
        assert_eq!(settings.try_get::<String>("", "scan").unwrap(), "off");
        txn.execute_unprepared("SET LOCAL enable_indexscan=off; SET LOCAL enable_bitmapscan=off").await.unwrap();
        let exact = txn.query_all_raw(statement).await.unwrap();
        assert_eq!(exact.len(), natural.len());
        let exact_ids: std::collections::HashSet<uuid::Uuid> = exact.iter().map(|row| row.try_get("", "id").unwrap()).collect();
        let recall = hits.iter().filter(|hit| exact_ids.contains(&hit.entity.id)).count() as f64 / 200.0;
        assert!(recall >= 0.95, "production recall@200={recall}; plan={plan}");
        txn.rollback().await.unwrap();
        // Hold the actual partition lock until concurrent index construction is waiting.
        let partition = ctx.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("SELECT tableoid::regclass::text AS name FROM entity_embeddings WHERE workspace_id='{}' LIMIT 1", fixture.workspace_id)))
            .await.unwrap().unwrap().try_get::<String>("", "name").unwrap();
        // Put adversarial near-identical vectors in another workspace of the same tenant/partition.
        let remainder: i32 = partition.strip_prefix("entity_embeddings_p").unwrap().parse().unwrap();
        let neighbor = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT id FROM (SELECT uuidv7() AS id FROM generate_series(1,4096)) candidates WHERE satisfies_hash_partition('entity_embeddings'::regclass,64,$1,id) LIMIT 1",
            [remainder.into()])).await.unwrap().unwrap().try_get::<uuid::Uuid>("", "id").unwrap();
        let mut neighbor_workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id)
            .one(&ctx.db).await.unwrap().unwrap().into_active_model();
        neighbor_workspace.id = Set(neighbor);
        neighbor_workspace.name = Set(format!("neighbor-{neighbor}"));
        neighbor_workspace.insert(&ctx.db).await.unwrap();
        let neighbor_ids: Vec<_> = (0..3000).map(|_| uuid::Uuid::now_v7()).collect();
        entities::Entity::insert_many(neighbor_ids.iter().map(|id| {
            let mut row = original.clone().into_active_model(); row.id=Set(*id); row.workspace_id=Set(neighbor); row
        })).exec(&ctx.db).await.unwrap();
        vectors::Entity::insert_many(neighbor_ids.iter().map(|id| vectors::ActiveModel {
            workspace_id:Set(neighbor), entity_id:Set(*id), tenant_id:Set(fixture.tenant_id),
            embedding:Set(Some(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;7])))
        })).exec(&ctx.db).await.unwrap();
        let same_partition = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT tableoid::regclass::text AS name FROM entity_embeddings WHERE workspace_id=$1 LIMIT 1", [neighbor.into()]))
            .await.unwrap().unwrap().try_get::<String>("", "name").unwrap();
        assert_eq!(same_partition, partition);
        let scoped = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let after = search::search_by_vector(&scoped, fixture.workspace_id, vec![1.0_f32;7], "", search::SearchQuery { limit:200, ..Default::default() }).await.unwrap();
        assert!(after.iter().all(|hit| hit.entity.workspace_id == fixture.workspace_id));
        let after_ids: std::collections::HashSet<_> = after.iter().map(|hit| hit.entity.id).collect();
        assert_eq!(after_ids, hits.iter().map(|hit| hit.entity.id).collect(), "neighbor vectors must not change the workspace graph results");
        let after_recall = after_ids.intersection(&exact_ids).count() as f64 / 200.0;
        assert_eq!(after_recall, recall);
        scoped.rollback().await.unwrap();
        workspace_workspaces::Entity::delete_by_id(neighbor).exec(&ctx.db).await.unwrap();
        ctx.db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY \"{index}\"")).await.unwrap();
        let blocker = ctx.db.begin().await.unwrap();
        blocker.execute_unprepared(&format!("LOCK TABLE {partition} IN SHARE MODE")).await.unwrap();
        let maintenance_db = ctx.db.clone();
        let maintenance = tokio::spawn(async move { entity_embeddings::maintain_workspace_indexes(&maintenance_db).await });
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let waiting = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1) AS waiting",
                    [format!("CREATE INDEX CONCURRENTLY \"{index}\"%").into()])).await.unwrap().unwrap().try_get::<bool>("", "waiting").unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("maintenance must reach the actual concurrent build lock");
        let (provider, _server) = fixed_width_provider(7).await;
        let reindex_db = ctx.db.clone();
        let workspace_id = fixture.workspace_id;
        let publication = tokio::spawn(async move { entity_embeddings::reindex_workspace(&reindex_db, workspace_id, &provider).await });
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id).one(&ctx.db).await.unwrap().unwrap();
                if workspace.embedding_reindexing { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("real reindex must fence publication while index build is blocked");
        blocker.rollback().await.unwrap();
        maintenance.await.unwrap().unwrap();
        let published = publication.await.unwrap().unwrap();
        assert!(published.failures.is_empty());
        assert_eq!(published.reindexed, 2001);
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(index.clone()).await.unwrap().unwrap().try_get::<bool>("", "indisvalid").unwrap());
        let stamped = workspace_workspaces::Entity::find_by_id(fixture.workspace_id).one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(stamped.embedding_dimensions, Some(7));
        assert!(!stamped.embedding_reindexing);
        // Exercise the real high-dimensional index lifecycle, not only generated SQL.
        vectors::Entity::update_many().col_expr(vectors::Column::Embedding,
            sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;4000])))
            .filter(vectors::Column::WorkspaceId.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingDimensions,
            sea_orm::sea_query::Expr::value(4000)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        let high_index = format!("ysr_hnsw_{}_d4000_nl2", fixture.workspace_id.simple());
        let legacy_index = format!("ysr_hnsw_{}_d4000", fixture.workspace_id.simple());
        let partition = ctx.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("SELECT tableoid::regclass::text AS name FROM entity_embeddings WHERE workspace_id='{}' LIMIT 1", fixture.workspace_id)))
            .await.unwrap().unwrap().try_get::<String>("", "name").unwrap();
        // Catalog cleanup must retire both abandoned high-dimensional index identities.
        for name in [&high_index, &legacy_index] {
            ctx.db.execute_unprepared(&format!("CREATE INDEX \"{name}\" ON {partition}(workspace_id)")).await.unwrap();
        }
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(high_index.clone()).await.unwrap().is_none());
        assert!(exists(legacy_index).await.unwrap().is_none());
        let graph = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT dimensions, octet_length(graph) AS bytes FROM entity_embedding_indexes WHERE workspace_id=$1",
            [fixture.workspace_id.into()])).await.unwrap().unwrap();
        assert_eq!(graph.try_get::<i32>("", "dimensions").unwrap(), 4000);
        assert!(graph.try_get::<i32>("", "bytes").unwrap() > 0);
        let tenant = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let positive = search::search_by_vector(&tenant, fixture.workspace_id, vec![1.0;4000], "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(positive.len(), 10);
        assert!(positive.iter().all(|hit| hit.distance.unwrap().abs() < 0.00001));
        let mut nearest = vec![1.0_f32;4000]; nearest[0] = 0.001;
        tenant.rollback().await.unwrap();
        vectors::Entity::update_many().col_expr(vectors::Column::Embedding,
            sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(nearest)))
            .filter(vectors::Column::EntityId.eq(ids[1])).exec(&ctx.db).await.unwrap();
        let invalidated = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT graph IS NOT NULL AND pending <> '{}'::jsonb AS queued FROM entity_embedding_indexes WHERE workspace_id=$1",
            [fixture.workspace_id.into()])).await.unwrap().unwrap();
        assert!(invalidated.try_get::<bool>("", "queued").unwrap());
        let tenant = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let mut signed = vec![1.0_f32;4000]; signed[0] = -1.0;
        let reranked = search::search_by_vector(&tenant, fixture.workspace_id, signed, "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(reranked.len(), 10);
        assert_eq!(reranked[0].entity.id, ids[1], "high-dimensional HNSW must retain the closest original cosine");
        assert!(reranked.iter().all(|hit| hit.entity.workspace_id == fixture.workspace_id));
        assert!(reranked.windows(2).all(|pair| pair[0].distance <= pair[1].distance));
        tenant.rollback().await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        let persisted = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT graph IS NOT NULL AND pending = '{}'::jsonb AS synchronized FROM entity_embedding_indexes WHERE workspace_id=$1",
            [fixture.workspace_id.into()])).await.unwrap().unwrap();
        assert!(persisted.try_get::<bool>("", "synchronized").unwrap());
        let tenant = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let mut signed = vec![1.0_f32;4000]; signed[0] = -1.0;
        let reloaded = search::search_by_vector(&tenant, fixture.workspace_id, signed, "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(reloaded[0].entity.id, ids[1], "persisted incremental update must survive graph reload");
        tenant.rollback().await.unwrap();
        let reader = ctx.db.begin().await.unwrap();
        reader.execute_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock_shared(hashtextextended('embedding-hnsw:' || $1::uuid::text, 0))",
            [fixture.workspace_id.into()])).await.unwrap();
        let delete_db = ctx.db.clone();
        let deleted_id = ids[1];
        let deletion = tokio::spawn(async move {
            let txn = delete_db.begin().await.unwrap();
            let pid = txn.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
                "SELECT pg_backend_pid() AS pid")).await.unwrap().unwrap().try_get::<i32>("", "pid").unwrap();
            vectors::Entity::delete_many().filter(vectors::Column::EntityId.eq(deleted_id)).exec(&txn).await.unwrap();
            txn.commit().await.unwrap();
            pid
        });
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let blocked = ctx.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE 'DELETE%entity_embeddings%') AS blocked"))
                    .await.unwrap().unwrap().try_get::<bool>("", "blocked").unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("vector deletion must wait for an active ANN reader");
        reader.rollback().await.unwrap();
        deletion.await.unwrap();
        let mut signed = vec![1.0_f32;4000]; signed[0] = -1.0;
        let after_delete = search::search_by_vector(&ctx.db, fixture.workspace_id, signed.clone(), "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(after_delete.len(), 10);
        assert!(after_delete.iter().all(|hit| hit.entity.id != deleted_id));
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        let after_reload = search::search_by_vector(&ctx.db, fixture.workspace_id, signed, "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(after_reload.len(), 10);
        assert!(after_reload.iter().all(|hit| hit.entity.id != deleted_id));
        vectors::ActiveModel {
            workspace_id: Set(fixture.workspace_id), entity_id: Set(deleted_id), tenant_id: Set(fixture.tenant_id),
            embedding: Set(Some(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;4000]))),
        }.insert(&ctx.db).await.unwrap();
        // 4096 dimensions use the same full-coordinate application graph.
        vectors::Entity::update_many().col_expr(vectors::Column::Embedding,
            sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;4096])))
            .filter(vectors::Column::WorkspaceId.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingDimensions,
            sea_orm::sea_query::Expr::value(4096)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(high_index.clone()).await.unwrap().is_none());
        assert!(exists(format!("ysr_hnsw_{}_d4096", fixture.workspace_id.simple())).await.unwrap().is_none());
        let graph = ctx.db.query_one_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "SELECT dimensions FROM entity_embedding_indexes WHERE workspace_id=$1 AND graph IS NOT NULL",
            [fixture.workspace_id.into()])).await.unwrap().unwrap();
        assert_eq!(graph.try_get::<i32>("", "dimensions").unwrap(), 4096);
        let tenant = yorishiro::db::begin_workspace(&ctx, fixture.tenant_id, fixture.workspace_id).await.unwrap();
        let hits = search::search_by_vector(&tenant, fixture.workspace_id, vec![1.0;4096], "", search::SearchQuery { limit:10, ..Default::default() }).await.unwrap();
        assert_eq!(hits.len(), 10);
        assert!(hits.iter().all(|hit| hit.distance.unwrap().abs() < 0.00001));
        tenant.rollback().await.unwrap();
        vectors::Entity::update_many().col_expr(vectors::Column::Embedding,
            sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;7])))
            .filter(vectors::Column::WorkspaceId.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingDimensions,
            sea_orm::sea_query::Expr::value(7)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(high_index).await.unwrap().is_none());
        // Publication in progress must retain the old graph even if metadata is changing.
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingReindexing,
            sea_orm::sea_query::Expr::value(true)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        let publication = ctx.db.begin().await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingDimensions,
            sea_orm::sea_query::Expr::value(13)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&publication).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), entity_embeddings::maintain_workspace_indexes(&ctx.db)).await.unwrap().unwrap();
        assert!(exists(index.clone()).await.unwrap().is_some());
        publication.rollback().await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingReindexing,
            sea_orm::sea_query::Expr::value(false)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        // A valid graph survives shrinkage and repeated maintenance.
        vectors::Entity::delete_many().filter(vectors::Column::EntityId.eq(ids[0])).exec(&ctx.db).await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(index.clone()).await.unwrap().is_some());
        // Widen all vectors and atomically published metadata before the next sweep.
        vectors::Entity::update_many().col_expr(vectors::Column::Embedding,
            sea_orm::sea_query::Expr::val(sea_orm::entity::prelude::PgVector::from(vec![1.0_f32;13])))
            .filter(vectors::Column::WorkspaceId.eq(fixture.workspace_id))
            .exec(&ctx.db).await.unwrap();
        workspace_workspaces::Entity::update_many().col_expr(workspace_workspaces::Column::EmbeddingDimensions,
            sea_orm::sea_query::Expr::value(13)).filter(workspace_workspaces::Column::Id.eq(fixture.workspace_id)).exec(&ctx.db).await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(index.clone()).await.unwrap().is_none());
        // Failed concurrent builds leave invalid indexes; recreate that catalog state using a failed unique build.
        let partition = sea_orm::ConnectionTrait::query_one_raw(&ctx.db, Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("SELECT tableoid::regclass::text AS name FROM entity_embeddings WHERE workspace_id='{}' LIMIT 1",fixture.workspace_id))).await.unwrap().unwrap().try_get::<String>("","name").unwrap();
        let bad = format!("ysr_hnsw_{}_d13", fixture.workspace_id.simple());
        sea_orm::ConnectionTrait::execute_unprepared(&ctx.db,&format!("DROP INDEX CONCURRENTLY \"{bad}\"")).await.unwrap();
        assert!(sea_orm::ConnectionTrait::execute_unprepared(&ctx.db,&format!("CREATE UNIQUE INDEX CONCURRENTLY \"{bad}\" ON {partition}(tenant_id)")).await.is_err());
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(bad.clone()).await.unwrap().unwrap().try_get::<bool>("", "indisvalid").unwrap(), "invalid promoted graph is rebuilt");
        // Attached partitions need not follow the original p0..p63 naming convention.
        ctx.db.execute_unprepared(&format!("ALTER TABLE {partition} RENAME TO \"expanded_\"\"p90\"" )).await.unwrap();
        ctx.db.execute_unprepared(&format!("DROP INDEX CONCURRENTLY \"{bad}\"" )).await.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(bad).await.unwrap().unwrap().try_get::<bool>("", "indisvalid").unwrap());
        ctx.db.execute_unprepared(&format!("ALTER TABLE \"expanded_\"\"p90\" RENAME TO {partition}" )).await.unwrap();
        // Deleting workspace metadata leaves no owned graph after the sweep.
        sea_orm::ConnectionTrait::execute_unprepared(&ctx.db,&format!("CREATE INDEX CONCURRENTLY \"{index}\" ON {partition}(workspace_id)")).await.unwrap();
        let deletion = ctx.db.begin().await.unwrap();
        workspace_workspaces::Entity::delete_by_id(fixture.workspace_id).exec(&deletion).await.unwrap();
        let (maintenance, committed) = tokio::join!(
            entity_embeddings::maintain_workspace_indexes(&ctx.db),
            deletion.commit(),
        );
        committed.unwrap();
        maintenance.unwrap();
        entity_embeddings::maintain_workspace_indexes(&ctx.db).await.unwrap();
        assert!(exists(index).await.unwrap().is_none());
    }).await;
}

async fn literal_search_corpus() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let definition = serde_json::from_value(note_definition()).unwrap();
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            definition,
            None,
            None,
        )
        .await
        .unwrap();
        let body = format!(
            "{} quarterly roadmap コネクタ導入 数据库迁移 ＭＣＰ ｺﾈｸﾀ ＷＩＤＴＨ tunnel-based a:b foo\"bar MCP AND NOT x (x 100%under_score",
            "背景説明と unrelated filler sentences。".repeat(100)
        );
        let mut matching = None;
        let mut corpus_ids = Vec::new();
        for title in [body, "unrelated grocery list".into()] {
            let entity = entity_entities::create(
                &ctx.db,
                fixture.workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({"title": title}),
                },
                None,
            )
            .await
            .unwrap();
            matching.get_or_insert(entity.id);
            corpus_ids.push(entity.id);
        }
        let sqlite = sea_orm::ConnectionTrait::get_database_backend(&ctx.db)
            == sea_orm::DatabaseBackend::Sqlite;
        // Both backends use this exact corpus; only documented SQLite limitations differ.
        for (text, postgres_hit, sqlite_hit) in [
            ("quarterly roadmap", true, true),
            ("コネクタ導入", true, true),
            ("導入", true, false),
            ("数据库", true, true),
            ("数据", true, false),
            ("MCP", true, true),
            ("コネクタ", true, true),
            ("ＭＣＰ", true, true),
            ("ｺﾈｸﾀ", true, true),
            ("WIDTH", true, false),
            ("ＷＩＤＴＨ", true, false),
            ("tunnel-based", true, true),
            ("a:b", true, true),
            ("foo\"bar", false, false),
            ("foo\\\"bar", true, true),
            ("MCP AND", true, true),
            ("NOT", true, true),
            ("xy", false, false),
            ("(x", true, false),
            ("100%under_score", true, true),
            ("missing-literal", false, false),
            ("100_other_score", false, false),
            ("\"", true, false),
            ("", false, false),
            ("   ", false, false),
        ] {
            let hits = search::search_by_vector(
                &ctx.db,
                fixture.workspace_id,
                vec![1.0_f32; 768],
                text,
                search::SearchQuery::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("literal query {text:?}: {error}"));
            let expected = if sqlite { sqlite_hit } else { postgres_hit };
            let mut ids: Vec<_> = hits.iter().map(|hit| hit.entity.id).collect();
            // PostgreSQL searches serialized JSON, whose structural quotes occur in every row.
            let mut expected_ids = if text == "\"" && !sqlite {
                corpus_ids.clone()
            } else if expected {
                vec![matching.unwrap()]
            } else {
                vec![]
            };
            ids.sort_unstable();
            expected_ids.sort_unstable();
            assert_eq!(ids, expected_ids, "literal query {text:?}, SQLite={sqlite}");
            assert!(hits.iter().all(|hit| hit.distance.is_none()));
        }
    })
    .await;
}

#[tokio::test]
async fn sqlite_literal_search_shared_corpus() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    literal_search_corpus().await;
}

#[tokio::test]
async fn postgres_literal_search_shared_corpus() {
    if !super::super::require_postgres_backend() {
        return;
    }
    literal_search_corpus().await;
}

/// Vector search must rank by cosine distance (closest first) and must not leak another workspace's entities into the results, even when that workspace's vector is a closer match.
#[tokio::test]
async fn search_by_vector_ranks_by_distance_and_stays_within_the_workspace() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let txn = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &txn,
            tenant_id,
            &format!("other-{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();

        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");
        let other_def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            tenant_id,
            other_workspace.id,
            other_def,
            None,
            None,
        )
        .await
        .expect("create schema in other workspace");

        let close = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "close match" }),
            },
            None,
        )
        .await
        .expect("create close entity");
        let far = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "far match" }),
            },
            None,
        )
        .await
        .expect("create far entity");
        let other_workspace_entity = entity_entities::create(
            &ctx.db,
            other_workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "belongs to another workspace" }),
            },
            None,
        )
        .await
        .expect("create entity in other workspace");

        fn unit_vector(axis: usize) -> Vec<f32> {
            let mut v = vec![0.0_f32; 768];
            v[axis] = 1.0;
            v
        }

        let query_vector = unit_vector(0);
        let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
            "/embeddings",
            axum::routing::post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    let data = body["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                        .map(|(index, text)| {
                            let axis = usize::from(text.as_str().unwrap().contains("far match"));
                            serde_json::json!({"index": index, "embedding": unit_vector(axis)})
                        })
                        .collect::<Vec<_>>();
                    axum::Json(serde_json::json!({"data": data}))
                },
            ),
        ))
        .await;
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
            base_url: server.url.clone(),
            api_key: "test-key".into(),
            model: "ranking".into(),
            dimensions: 768,
            send_dimensions_param: false,
            query_instruction: String::new(),
            document_prefix: String::new(),
        });
        for (workspace, entity) in [
            (workspace_id, close.id),
            (workspace_id, far.id),
            (other_workspace.id, other_workspace_entity.id),
        ] {
            let record = entity_entities::get(&ctx.db, workspace, entity)
                .await
                .unwrap();
            entity_embeddings::sync_embedding_for_record(&ctx.db, workspace, &record, &provider)
                .await
                .unwrap();
        }

        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            query_vector,
            "match",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 2, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, close.id, "closest vector ranks first");
        assert_eq!(hits[1].entity.id, far.id);
        assert!(hits[0].distance.unwrap() < hits[1].distance.unwrap());
        assert!(
            hits.iter().all(|h| h.entity.workspace_id == workspace_id),
            "no cross-workspace leakage: {hits:?}"
        );
    })
    .await;
}

async fn fixed_width_provider(
    width: usize,
) -> (OpenAiCompatibleProvider, crate::http_stub::HttpStub) {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            let data = body["input"].as_array().unwrap().iter().enumerate()
                .map(|(index, _)| serde_json::json!({"index": index, "embedding": vec![1.0_f32; width]}))
                .collect::<Vec<_>>();
            axum::Json(serde_json::json!({"data": data}))
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "fixed-width".into(),
        dimensions: width,
        send_dimensions_param: false,
        query_instruction: String::new(),
        document_prefix: String::new(),
    });
    (provider, server)
}

fn model_vector(model: &str) -> Vec<f32> {
    let mut vector = vec![0.0; 768];
    for (value, byte) in vector.iter_mut().zip(model.bytes()) {
        *value = byte as f32 / 255.0;
    }
    vector
}

async fn model_provider(
    model: &'static str,
) -> (OpenAiCompatibleProvider, crate::http_stub::HttpStub) {
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| async move {
            let data = body["input"].as_array().unwrap().iter().enumerate()
                .map(|(index, _)| serde_json::json!({"index": index, "embedding": model_vector(model)}))
                .collect::<Vec<_>>();
            axum::Json(serde_json::json!({"data": data}))
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: false,
        query_instruction: String::new(),
        document_prefix: String::new(),
    });
    (provider, server)
}

/// A workspace stamped with a dimension count (`workspace_workspaces.embedding_dimensions`) must refuse a sync whose provider produces a different width, rather than writing a vector that would silently break every future search over that workspace with a dimension-mismatch error naming neither the entity nor the write that caused it.
#[tokio::test]
async fn sync_embedding_refuses_a_vector_that_does_not_match_the_workspace_stamp() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let (stamp_provider, _stamp_server) = fixed_width_provider(768).await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _server) = fixed_width_provider(1024).await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            workspace_id,
            &record,
            &mismatched_provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored = entity_embeddings::Entity::find()
            .filter(yorishiro::models::_entities::entity_embeddings::Column::EntityId.eq(entity.id))
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(
            !stored,
            "the refused write must not have created an embedding row: {stored:?}"
        );
    })
    .await;
}

async fn gated_model_provider(
    model: &'static str,
) -> (
    std::sync::Arc<OpenAiCompatibleProvider>,
    crate::http_stub::HttpStub,
    std::sync::Arc<tokio::sync::Notify>,
    std::sync::Arc<tokio::sync::Semaphore>,
) {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let handler_entered = entered.clone();
    let handler_release = release.clone();
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let calls = calls.clone();
            let entered = handler_entered.clone();
            let release = handler_release.clone();
            async move {
                let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if call == 1 {
                    entered.notify_one();
                    release.acquire().await.expect("release embedding response").forget();
                }
                let data = body["input"].as_array().unwrap().iter().enumerate()
                    .map(|(index, _)| serde_json::json!({"index": index, "embedding": model_vector(model)}))
                    .collect::<Vec<_>>();
                axum::Json(serde_json::json!({"data": data}))
            }
        }),
    )).await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: model.into(),
        dimensions: 768,
        send_dimensions_param: false,
        query_instruction: String::new(),
        document_prefix: String::new(),
    });
    (std::sync::Arc::new(provider), server, entered, release)
}

/// A workspace stamped with a model name (`workspace_workspaces.embedding_model`) must refuse a sync whose provider reports a different model, even though both produce 768-dimensional vectors and the dimension check above cannot see any difference: this is exactly the nomic/multilingual-e5-base coexistence `entity_entities.embedding vector(768)` allows, and the case that check exists to catch.
#[tokio::test]
async fn sync_embedding_refuses_a_vector_from_a_different_model_than_the_workspace_stamp() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let (stamp_provider, _stamp_server) =
            model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "will not get an embedding" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _mismatched_provider_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            workspace_id,
            &record,
            &mismatched_provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result:?}"
        );

        let stored = entity_embeddings::Entity::find()
            .filter(yorishiro::models::_entities::entity_embeddings::Column::EntityId.eq(entity.id))
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(
            !stored,
            "the refused write must not have created an embedding row: {stored:?}"
        );
    })
    .await;
}

/// A workspace with no stamp of its own inherits its tenant's `embedding_model`/`embedding_dimensions` as the effective model check target and dimension target.
/// If the tenant's value and the deployment default are the same, deleting the join would pass this test, so the tenant is set to a different model than the deployment default: removing the join would then cause the test to use the deployment default instead of the tenant's, and the provider mismatch would be caught by the model check.
#[tokio::test]
async fn sync_embedding_resolves_the_tenant_tier_of_the_embedding_chain() {
    if !super::super::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        // No public operation sets tenant embedding stamps; inject the legacy fallback tier.
        sea_orm::ActiveModelTrait::update(
            tenant_tenants::ActiveModel {
                id: sea_orm::ActiveValue::Set(fixture.tenant_id),
                embedding_model: sea_orm::ActiveValue::Set(Some(
                    "nomic-ai/nomic-embed-text-v1.5".into(),
                )),
                embedding_dimensions: sea_orm::ActiveValue::Set(Some(768)),
                ..Default::default()
            },
            &ctx.db,
        )
        .await
        .expect("stamp tenant embedding fallback");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "resolves from tenant tier" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, fixture.workspace_id, entity.id)
            .await
            .expect("get entity record");

        // The tenant is stamped with nomic-embed-text-v1.5, so the effective model must be that.
        // Using the same model provider should succeed.
        let (matching_provider, _matching_provider_server) =
            model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record,
            &matching_provider,
        )
        .await;
        assert!(result.is_ok(), "result: {result:?}");

        // Verify the embedding was written.
        let stored = entity_embeddings::Entity::find()
            .filter(yorishiro::models::_entities::entity_embeddings::Column::EntityId.eq(entity.id))
            .filter(
                yorishiro::models::_entities::entity_embeddings::Column::Embedding.is_not_null(),
            )
            .exists(&ctx.db)
            .await
            .expect("query embedding");
        assert!(stored, "embedding must have been written");

        // A different model should be refused by the inherited model check.
        let entity2 = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong model, should be refused" }),
            },
            None,
        )
        .await
        .expect("create second entity");
        let record2 = entity_entities::get(&ctx.db, fixture.workspace_id, entity2.id)
            .await
            .expect("get entity record");

        let (mismatched_provider, _mismatched_provider_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let result2 = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record2,
            &mismatched_provider,
        )
        .await;
        assert!(
            matches!(result2, Err(YorishiroError::ValidationFailed { .. })),
            "result: {result2:?}"
        );
    })
    .await;
}

/// A workspace inheriting its tenant's `embedding_dimensions` must refuse a sync whose provider
/// produces a different width. The dimension check runs before the model check, so a 768 provider
/// against a 1024 tenant is rejected on width; the `contains("1024")` assertion in the test
/// proves it was the dimension and not the model check that fired.
#[tokio::test]
async fn sync_embedding_resolves_the_tenant_dimension_tier() {
    if !super::super::require_postgres_backend() {
        return;
    }
    crate::requests::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        // No public operation sets tenant embedding stamps; inject the legacy fallback tier.
        sea_orm::ActiveModelTrait::update(
            tenant_tenants::ActiveModel {
                id: sea_orm::ActiveValue::Set(fixture.tenant_id),
                embedding_model: sea_orm::ActiveValue::Set(Some(
                    "nomic-ai/nomic-embed-text-v1.5".into(),
                )),
                embedding_dimensions: sea_orm::ActiveValue::Set(Some(1024)),
                ..Default::default()
            },
            &ctx.db,
        )
        .await
        .expect("stamp tenant embedding fallback");
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            def,
            None,
            None,
        )
        .await
        .expect("create schema");

        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "wrong dimension from tenant" }),
            },
            None,
        )
        .await
        .expect("create entity");
        let record = entity_entities::get(&ctx.db, fixture.workspace_id, entity.id)
            .await
            .expect("get entity record");
        // Provider is 768-dimensional, not the workspace's 1024: the dimension check fires
        // first and rejects the write before the model check is reached.
        let (provider, _server) = fixed_width_provider(768).await;
        let result = entity_embeddings::sync_embedding_for_record(
            &ctx.db,
            fixture.workspace_id,
            &record,
            &provider,
        )
        .await;

        assert!(
            matches!(result, Err(YorishiroError::ValidationFailed { .. })),
            "expected ValidationFailed for dimension mismatch: {result:?}"
        );
        assert!(
            result.unwrap_err().to_string().contains("1024"),
            "error message must name the expected dimension count 1024"
        );
    })
    .await;
}

/// Two concurrent `reindex_workspace_with_lock` calls against the same workspace with different
/// providers must serialize via the advisory lock: the second waits for the first, and the final
/// stamp and per-row vector provenance agree.
///
/// Without the lock acquisition in `src/db.rs`, the two concurrent calls race and the final stamp does
/// not match the vectors actually stored, causing this assertion to fail.
#[tokio::test]
async fn concurrent_reindex_runs_serialize_and_consistent_after_lock() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create entities to reindex.
        let e1 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Pre-stamp with an old model to simulate a workspace that needs reindexing.
        let (stamp_provider, _stamp_server) = model_provider("old-model").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");

        let candidate_ids: Vec<uuid::Uuid> = vec![e1.id, e2.id];

        let (provider1, _provider1_server, entered, release) =
            gated_model_provider("nomic-ai/nomic-embed-text-v1.5").await;
        let (provider2, _provider2_server) =
            model_provider("intfloat/multilingual-e5-base").await;
        let provider2 = std::sync::Arc::new(provider2);

        let provider1_clone = provider1.clone();
        let ctx1 = ctx.clone();
        let h1 = tokio::spawn(async move {
            yorishiro::db::reindex_workspace_with_lock(
                &ctx1,
                workspace_id,
                provider1_clone.as_ref(),
            )
            .await
            .expect("reindex 1 join")
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await.expect("first reindex reached its second embedding request");
        let ctx2 = ctx.clone();
        let provider2_clone = provider2.clone();
        let h2 = tokio::spawn(async move {
            yorishiro::db::reindex_workspace_with_lock(
                &ctx2,
                workspace_id,
                provider2_clone.as_ref(),
            )
            .await
            .expect("reindex 2 join")
        });

        // Inspect the actual lock wait, not elapsed time; this is a database-lock regression.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                #[derive(sea_orm::FromQueryResult)]
                struct LockWait { waiting: bool }
                let waiting = LockWait::find_by_statement(Statement::from_sql_and_values(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS waiting",
                    [yorishiro::db::workspace_reindex_lock_key(workspace_id).into()],
                )).one(&ctx.db).await.expect("observe workspace advisory lock")
                    .expect("lock observation row").waiting;
                if waiting { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("second reindex waits for this workspace's advisory lock");
        release.add_permits(1);

        let outcome1 = h1.await.expect("handler 1 finished");
        let outcome2 = h2.await.expect("handler 2 finished");

        assert!(
            outcome1.failures.is_empty(),
            "first reindex failed: {}",
            outcome1
                .failures
                .iter()
                .map(|f| f.error.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(
            outcome2.failures.is_empty(),
            "second reindex failed: {}",
            outcome2
                .failures
                .iter()
                .map(|f| f.error.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // The final workspace stamp must match the provider that won the lock race.
        // We check that the stamp and the stored vectors agree: whichever provider won,
        // both the stamp and every entity's embedding must point to it.
        let final_model = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");

        let winner = if final_model == "nomic-ai/nomic-embed-text-v1.5" {
            provider1.as_ref()
        } else if final_model == "intfloat/multilingual-e5-base" {
            provider2.as_ref()
        } else {
            panic!("unexpected model stamp: {final_model:?}");
        };

        // Verify every entity's embedding matches the winning provider's vector.
        // `PgVector::data()` returns the raw `Vec<f32>` bytes.
        // The float comparison is safe: `*b as f32 / 255.0` round-trips exactly through
        // PostgreSQL `real` (32-bit float), so `assert_eq!` is valid.
        for entity_id in &candidate_ids {
            let stored_vector: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find().filter(yorishiro::models::_entities::entity_embeddings::Column::EntityId.eq(*entity_id))
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            let expected = model_vector(&winner.model_name());
            assert_eq!(
                stored_vector.as_ref().map(|v| v.as_slice()),
                Some(expected.as_slice()),
                "entity {entity_id} embedding must match winner {final_model:?}"
            );
        }
    })
    .await;
}

/// `reindex_workspace` must overwrite existing vectors with the new model's embeddings: if a
/// provider's `embed_batch` impl is replaced and a reindex runs, every entity's embedding must
/// change from the old model's vector to the new model's vector.
///
/// This proves both halves: `assert_ne!` against the old model's vector (the row was not left
/// untouched) and `assert_eq!` against the new model's vector (the new embedding was actually
/// written). Without the `assert_ne!`, a broken `reindex` that skipped `embed_as` and left
/// the old vector in place would still pass the `assert_eq!` on the existing data; without the
/// `assert_eq!`, a corrupted or zeroed row would pass `assert_ne!` on its own.
#[tokio::test]
async fn reindex_overwrites_existing_entity_embeddings() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create entities.
        let e1 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity one" }),
            },
            None,
        )
        .await
        .expect("create entity 1");
        let e2 = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "entity two" }),
            },
            None,
        )
        .await
        .expect("create entity 2");

        // Embed with the old model provider.
        let (old_provider, _old_provider_server) = model_provider("old-model-a").await;
        for entity in [&e1, &e2] {
            let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
                .await
                .expect("get entity record");
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                &old_provider,
            )
            .await
            .expect("embed with old model");
        }

        // Verify the old vectors are stored.
        // Catalog queries verify physical partitioning, which the entity API cannot describe.
        let layout = sea_orm::ConnectionTrait::query_one_raw(&ctx.db, Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT count(*) AS partitions FROM pg_inherits WHERE inhparent = 'entity_embeddings'::regclass",
        )).await.unwrap().unwrap();
        assert_eq!(layout.try_get::<i64>("", "partitions").unwrap(), 64);
        let policy = sea_orm::ConnectionTrait::query_one_raw(&ctx.db, Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT qual FROM pg_policies WHERE tablename = 'entity_embeddings' AND policyname = 'tenant_isolation'",
        )).await.unwrap().unwrap();
        let condition: String = policy.try_get("", "qual").unwrap();
        assert!(condition.contains("tenant_id") && condition.contains("app.current_tenant"));
        assert!(!condition.contains("entity_entities"));
        let old_v1 = model_vector(&old_provider.model_name());
        for entity_id in [e1.id, e2.id] {
            use yorishiro::models::_entities::entity_embeddings as vectors;
            let owner = vectors::Entity::find_by_id((workspace_id, entity_id))
                .select_only().column(vectors::Column::TenantId)
                .into_tuple::<uuid::Uuid>().one(&ctx.db).await.unwrap().unwrap();
            assert_eq!(owner, tenant_id);
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find()
                    .filter(
                        yorishiro::models::_entities::entity_embeddings::Column::EntityId
                            .eq(entity_id),
                    )
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            assert_eq!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(old_v1.as_slice()),
                "initial embedding must match the old model"
            );
        }

        // Stamp the workspace with the old model so reindex considers it "in sync" — but
        // reindex's entire purpose is to re-embed regardless, so this stamp is only there
        // to make the test realistic.
        let (stamp_provider, _stamp_server) = model_provider("old-model-a").await;
        entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &stamp_provider)
            .await
            .expect("stamp workspace through reindex");

        // Reindex with the new model provider.
        let (new_provider, _new_provider_server) = model_provider("new-model-b").await;
        let outcome = yorishiro::models::entity_embeddings::reindex_workspace(
            &ctx.db,
            workspace_id,
            &new_provider,
        )
        .await
        .expect("reindex");

        if !outcome.failures.is_empty() {
            panic!("reindex had {} failure(s)", outcome.failures.len());
        }
        assert_eq!(outcome.reindexed, 2, "both entities must be reindexed");

        // Stamp and vectors must now agree on the new model.
        let final_model = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("query final workspace")
            .expect("workspace exists")
            .embedding_model
            .expect("workspace must be stamped with a model");
        assert_eq!(
            final_model, "new-model-b",
            "workspace stamp must reflect new model"
        );

        let new_v = model_vector(&new_provider.model_name());

        for entity_id in [e1.id, e2.id] {
            let stored: Option<sea_orm::entity::prelude::PgVector> = {
                #[derive(sea_orm::FromQueryResult)]
                struct Row {
                    embedding: Option<sea_orm::entity::prelude::PgVector>,
                }
                entity_embeddings::Entity::find()
                    .filter(
                        yorishiro::models::_entities::entity_embeddings::Column::EntityId
                            .eq(entity_id),
                    )
                    .select_only()
                    .column(yorishiro::models::_entities::entity_embeddings::Column::Embedding)
                    .into_model::<Row>()
                    .one(&ctx.db)
                    .await
                    .expect("query embedding")
                    .and_then(|r| r.embedding)
            };
            assert_eq!(
                stored.as_ref().map(|v| v.as_slice()),
                Some(new_v.as_slice()),
                "entity {entity_id} embedding must match new model {final_model:?}"
            );
        }
    })
    .await;
}

/// Gated PostgreSQL literal text search includes an unembedded entity when its data contains `query_text`; a nonmatching entity must not appear.
#[tokio::test]
async fn search_by_vector_falls_back_to_trigram_for_unembedded_entities() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        let matching = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create matching entity");
        entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "completely unrelated grocery list" }),
            },
            None,
        )
        .await
        .expect("create unrelated entity");
        // Neither entity has an embedding: both rely entirely on selective literal text matches.

        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 1, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);
        assert!(
            hits[0].distance.is_none(),
            "trigram-only hit has no distance"
        );
    })
    .await;
}

/// The FTS5 fallback on SQLite surfaces an entity whose `data` matches quoted FTS5 terms from `query_text`, when
/// the entity has no embedding; an entity with neither an embedding nor a MATCH match must not
/// appear.
///
/// On SQLite, the search function's trigram half (pg_trgm on PostgreSQL) is replaced by an
/// FTS5 query against `entity_fts` with `tokenize='trigram'`. This test boots
/// against a SQLite file database to confirm the FTS5 path works end to end, including schema
/// creation, entity insertion, and trigger-driven index updates.
#[tokio::test]
async fn search_by_vector_falls_back_to_fts5_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");

        // Create an entity whose title contains the search phrase.
        let matching = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "quarterly roadmap review" }),
            },
            None,
        )
        .await
        .expect("create matching entity");

        // Create an entity whose title does not match.
        entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "completely unrelated grocery list" }),
            },
            None,
        )
        .await
        .expect("create unrelated entity");

        // Neither entity has an embedding (SQLite has no embedding column), so both rely on
        // the FTS5 fallback path.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector");

        assert_eq!(hits.len(), 1, "hits: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);
        assert!(hits[0].distance.is_none(), "FTS5-only hit has no distance");

        // Modify the entity's data so the old search phrase no longer matches, then
        // confirm a search for the new phrase finds it.
        let rec = entity_entities::get(&ctx.db, workspace_id, matching.id)
            .await
            .expect("fetch entity for update");

        entity_entities::update(
            &ctx.db,
            workspace_id,
            entity_entities::UpdateEntityInput {
                id: rec.id,
                data: serde_json::json!({ "title": "quarterly board meeting notes" }),
                updated_by: None,
                expected_updated_at: None,
            },
        )
        .await
        .expect("update entity");

        // Old phrase should no longer match.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly roadmap",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector after update");
        assert_eq!(
            hits.len(),
            0,
            "old phrase must not match after update: {hits:?}"
        );

        // New phrase should find it.
        let hits = search::search_by_vector(
            &ctx.db,
            workspace_id,
            vec![0.0_f32; 768],
            "quarterly board meeting",
            search::SearchQuery::default(),
        )
        .await
        .expect("search_by_vector after update");
        assert_eq!(hits.len(), 1, "new phrase must match: {hits:?}");
        assert_eq!(hits[0].entity.id, matching.id);

        entity_entities::delete(&ctx.db, workspace_id, matching.id)
            .await
            .expect("delete entity");

        let after_delete = entity_entities::get(&ctx.db, workspace_id, matching.id).await;
        assert!(after_delete.is_err(), "deleted entity must not be found");
    })
    .await;
}

/// SQLite end-to-end: entity create → document embed via test provider → persisted vector → vector search hit.
///
/// Drives the public `sync_embedding_for_record` directly, not the worker.
/// `tests/requests/search.rs` covers the worker and the HTTP search on both backends.
#[tokio::test]
async fn sqlite_document_embed_persist_and_search() {
    if !super::super::require_sqlite_backend() {
        return;
    }

    boot_request::<App, _, _>(|request, ctx| async move {
            let crate::requests::fixtures::TestTenant { tenant_id, workspace_id, .. } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;

            let def = serde_json::from_value(note_definition()).expect("parse definition");
            schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
                .await
                .expect("create schema");

            // Create entity to embed.
            let entity = entity_entities::create(
                &ctx.db,
                workspace_id,
                entity_entities::CreateEntityInput {
                    schema_name: "note".into(),
                    entity_type: "note".into(),
                    data: serde_json::json!({ "title": "quarterly roadmap review" }),
                },
                None,
            )
            .await
            .expect("create entity");

            // Exercise the production HTTP provider and persistence path.
            let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
                .await
                .expect("get entity record");
            let (provider, _provider_server) = model_provider("test-model").await;
            entity_embeddings::sync_embedding_for_record(
                &ctx.db,
                workspace_id,
                &record,
                &provider,
            )
            .await
            .expect("embed entity");

            // Verify the embedding was written to the shared table.
            let has_embedding = entity_embeddings::Entity::find().filter(yorishiro::models::_entities::entity_embeddings::Column::EntityId.eq(entity.id))
                .select_only()
                .expr_as(
                    sea_orm::sea_query::Expr::col(
                        yorishiro::models::_entities::entity_embeddings::Column::Embedding,
                    ).is_not_null(),
                    "has_embedding",
                )
                .into_tuple::<bool>()
                .one(&ctx.db)
                .await
                .expect("query embedding");
            assert_eq!(has_embedding, Some(true), "embedding must have been written");

            // Search by vector should find the entity.
            let hits = search::search_by_vector(
                &ctx.db,
                workspace_id,
                model_vector(&provider.model_name()),
                "quarterly roadmap",
                search::SearchQuery::default(),
            )
            .await
            .expect("search_by_vector");
            assert_eq!(hits.len(), 1, "vector search must find the embedded entity, hits: {hits:?}");
            assert_eq!(hits[0].entity.id, entity.id);
            assert!(
                hits[0].distance.is_some(),
                "vector hit must have a distance"
            );
            #[cfg(feature = "enterprise")]
            {
                use loco_rs::task::{Task, Vars};
                use yorishiro::edition::ee::tasks::sqlite_ann_benchmark::SqliteAnnBenchmark;
                SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                ])).await.expect("benchmark stored embedding without a vec0 table");
                ctx.db.execute_unprepared("CREATE VIRTUAL TABLE entity_embeddings_vec0 USING vec0(entity_id TEXT PRIMARY KEY, workspace_id TEXT PARTITION KEY, embedding FLOAT[768] distance_metric=cosine)")
                    .await.unwrap();
                let unrelated_workspace = uuid::Uuid::now_v7();
                use sea_orm::sea_query::{Alias, Expr, Query};
                // Fault injection populates the optional vec0 index, not application-owned rows.
                for (id, workspace) in (0..11)
                    .map(|_| (uuid::Uuid::now_v7(), unrelated_workspace))
                    .chain(std::iter::once((entity.id, workspace_id)))
                {
                    let insert = Query::insert()
                        .into_table(Alias::new("entity_embeddings_vec0"))
                        .columns([Alias::new("entity_id"), Alias::new("workspace_id"), Alias::new("embedding")])
                        .values_panic([
                            id.to_string().into(), workspace.to_string().into(),
                            yorishiro::db::sqlite_vec_blob(&model_vector(&provider.model_name())).into(),
                        ]).to_owned();
                    ctx.db.execute_raw(ctx.db.get_database_backend().build(&insert)).await.unwrap();
                }
                SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                    ("min_entities".into(), "0".into()),
                    ("max_latency_ms".into(), "0".into()),
                ])).await.expect("benchmark real vec0 KNN with UUID keys and workspace partition");
                let delete = Query::delete()
                    .from_table(Alias::new("entity_embeddings_vec0"))
                    .and_where(Expr::col(Alias::new("entity_id")).eq(entity.id.to_string()))
                    .to_owned();
                ctx.db.execute_raw(ctx.db.get_database_backend().build(&delete)).await.unwrap();
                let error = SqliteAnnBenchmark.run(&ctx, &Vars::from_cli_args(vec![
                    ("workspace_id".into(), workspace_id.to_string()),
                    ("min_entities".into(), "0".into()),
                    ("max_latency_ms".into(), "0".into()),
                ])).await.expect_err("an unpopulated workspace partition must not look like a fast benchmark");
                assert!(error.to_string().contains("returned no entities"), "{error}");
            }
    }).await;
}

/// A reindex re-embeds entities that already have a vector, which is the whole point of switching models.
/// Backend-neutral: runs on whichever backend `DATABASE_URL` names.
#[tokio::test]
async fn reindex_replaces_existing_vectors_and_restamps_the_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let crate::requests::fixtures::TestTenant {
            tenant_id,
            workspace_id,
            ..
        } = crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        let def = serde_json::from_value(note_definition()).expect("parse definition");
        schema_schemas::create_schema(&ctx.db, tenant_id, workspace_id, def, None, None)
            .await
            .expect("create schema");
        let entity = entity_entities::create(
            &ctx.db,
            workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({ "title": "already embedded" }),
            },
            None,
        )
        .await
        .expect("create entity");

        let (first, _first_server) = model_provider("model-a").await;
        let record = entity_entities::get(&ctx.db, workspace_id, entity.id)
            .await
            .expect("get entity");
        entity_embeddings::sync_embedding_for_record(&ctx.db, workspace_id, &record, &first)
            .await
            .expect("first embed");

        let (second, _second_server) = model_provider("model-b").await;
        let outcome = entity_embeddings::reindex_workspace(&ctx.db, workspace_id, &second)
            .await
            .expect("reindex");

        assert_eq!(outcome.total, 1);
        assert_eq!(outcome.reindexed, 1);
        assert!(
            outcome.failures.is_empty(),
            "an entity that already has a vector must be reindexed, not reported missing"
        );
        let stamped = workspace_workspaces::Entity::find_by_id(workspace_id)
            .one(&ctx.db)
            .await
            .expect("read workspace")
            .expect("workspace row");
        assert_eq!(stamped.embedding_model.as_deref(), Some("model-b"));
        use yorishiro::models::_entities::entity_embeddings as vectors;
        let scope = vectors::Entity::find_by_id((workspace_id, entity.id))
            .select_only()
            .columns([vectors::Column::WorkspaceId, vectors::Column::TenantId])
            .into_tuple::<(uuid::Uuid, uuid::Uuid)>()
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(scope, (workspace_id, tenant_id));
    })
    .await;
}

/// Width switching uses the actual vectors rather than a fixed registry of storage tables.
#[tokio::test]
async fn reindex_switches_between_arbitrary_widths() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let fixture =
            crate::requests::fixtures::TestTenant::create(&request, &ctx, Default::default()).await;
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            fixture.workspace_id,
            serde_json::from_value(note_definition()).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let entity = entity_entities::create(
            &ctx.db,
            fixture.workspace_id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title": "arbitrary widths"}),
            },
            None,
        )
        .await
        .unwrap();
        let transaction = sea_orm::TransactionTrait::begin(&ctx.db).await.unwrap();
        let other_workspace = yorishiro::models::tenancy::create_workspace(
            &transaction,
            fixture.tenant_id,
            &format!("other-width-{}", uuid::Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
        schema_schemas::create_schema(
            &ctx.db,
            fixture.tenant_id,
            other_workspace.id,
            serde_json::from_value(note_definition()).unwrap(),
            None,
            None,
        )
        .await
        .unwrap();
        let other = entity_entities::create(
            &ctx.db,
            other_workspace.id,
            entity_entities::CreateEntityInput {
                schema_name: "note".into(),
                entity_type: "note".into(),
                data: serde_json::json!({"title":"other workspace"}),
            },
            None,
        )
        .await
        .unwrap();
        let (other_provider, _other_server) = fixed_width_provider(11).await;
        entity_embeddings::reindex_workspace(&ctx.db, other_workspace.id, &other_provider)
            .await
            .unwrap();
        for width in [7, 4096, 13, 7] {
            let (provider, _server) = fixed_width_provider(width).await;
            let outcome =
                entity_embeddings::reindex_workspace(&ctx.db, fixture.workspace_id, &provider)
                    .await
                    .unwrap();
            assert!(outcome.failures.is_empty());
            assert_eq!(outcome.reindexed, 1);
            let workspace = workspace_workspaces::Entity::find_by_id(fixture.workspace_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(workspace.embedding_dimensions, Some(width as i32));
            use sea_orm::sea_query::{Alias, Expr, Func};
            use yorishiro::models::_entities::entity_embeddings as embeddings;
            let length = Func::cust(Alias::new(
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    "length"
                } else {
                    "vector_dims"
                },
            ))
            .arg(Expr::col(embeddings::Column::Embedding));
            let stored = embeddings::Entity::find()
                .filter(embeddings::Column::EntityId.eq(entity.id))
                .select_only()
                .expr(length)
                .into_tuple::<i32>()
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let expected = if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                width * std::mem::size_of::<f32>()
            } else {
                width
            };
            assert_eq!(stored, expected as i32);
            assert_eq!(
                embeddings::Entity::find()
                    .filter(embeddings::Column::EntityId.is_in([entity.id, other.id]))
                    .count(&ctx.db)
                    .await
                    .unwrap(),
                2
            );
            let other_length = Func::cust(Alias::new(
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    "length"
                } else {
                    "vector_dims"
                },
            ))
            .arg(Expr::col(embeddings::Column::Embedding));
            let retained = embeddings::Entity::find()
                .filter(embeddings::Column::EntityId.eq(other.id))
                .select_only()
                .expr(other_length)
                .into_tuple::<i32>()
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                retained,
                if ctx.db.get_database_backend() == sea_orm::DatabaseBackend::Sqlite {
                    11 * std::mem::size_of::<f32>() as i32
                } else {
                    11
                }
            );
            let hits = search::search_by_vector(
                &ctx.db,
                fixture.workspace_id,
                vec![1.0; width],
                "arbitrary",
                search::SearchQuery::default(),
            )
            .await
            .unwrap();
            assert_eq!(hits[0].entity.id, entity.id);
        }
    })
    .await;
}
