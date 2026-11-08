use std::collections::HashMap;

use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Statement, TransactionTrait,
};
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::entity_embeddings;

const FORMAT: &str = "usearch-2.26.6-l2-f32";
pub(crate) const CANDIDATE_LIMIT: usize = 1000;

fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|&x| f64::from(x).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm == 0.0 {
        return vector.to_vec();
    }
    vector
        .iter()
        .map(|&x| (f64::from(x) / norm) as f32)
        .collect()
}

fn build(
    rows: Vec<entity_embeddings::Model>,
    dimensions: usize,
) -> Result<(Vec<u8>, Vec<u8>), YorishiroError> {
    let graph = new_index(dimensions)?;
    graph.reserve(rows.len()).internal()?;
    let mut keys = Vec::with_capacity(rows.len());
    for row in rows {
        let vector = row
            .embedding
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("missing index vector")))?;
        super::validate_vector(vector.as_slice(), dimensions)?;
        graph
            .add(keys.len() as u64, &normalized(vector.as_slice()))
            .internal()?;
        keys.push(row.entity_id);
    }
    save(&graph, &keys)
}

fn new_index(dimensions: usize) -> Result<Index, YorishiroError> {
    Index::new(&IndexOptions {
        dimensions,
        metric: MetricKind::L2sq,
        quantization: ScalarKind::F32,
        connectivity: 32,
        expansion_add: 200,
        expansion_search: 2000,
        multi: false,
    })
    .internal()
}

fn save(graph: &Index, keys: &[Uuid]) -> Result<(Vec<u8>, Vec<u8>), YorishiroError> {
    let mut bytes = vec![0; graph.serialized_length()];
    graph.save_to_buffer(&mut bytes).internal()?;
    // USearch owns vector serialization; this second blob maps its u64 labels to entity UUIDs.
    Ok((bytes, serde_json::to_vec(keys).internal()?))
}

async fn changes(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    pending: serde_json::Value,
) -> Result<(Vec<Uuid>, Vec<entity_embeddings::Model>), YorishiroError> {
    let ids = pending
        .as_object()
        .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("invalid HNSW pending IDs")))?
        .keys()
        .map(|id| Uuid::parse_str(id).internal())
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok((ids, Vec::new()));
    }
    let rows = entity_embeddings::Entity::find()
        .filter(entity_embeddings::Column::WorkspaceId.eq(workspace_id))
        .filter(entity_embeddings::Column::EntityId.is_in(ids.clone()))
        .filter(entity_embeddings::Column::Embedding.is_not_null())
        .order_by_asc(entity_embeddings::Column::EntityId)
        .all(conn)
        .await
        .internal()?;
    Ok((ids, rows))
}

fn load(
    graph: &[u8],
    vectors: Vec<u8>,
    dimensions: usize,
    ids: Vec<Uuid>,
    rows: Vec<entity_embeddings::Model>,
) -> Result<(Index, Vec<Uuid>), YorishiroError> {
    let index = new_index(dimensions)?;
    index.load_from_buffer(graph).internal()?;
    index.change_expansion_search(2000);
    // Reused nodes need the same exploration budget as searches after large direction changes.
    index.change_expansion_add(2000);
    let mut keys: Vec<Uuid> = serde_json::from_slice(&vectors).internal()?;
    if ids.is_empty() && rows.is_empty() {
        return Ok((index, keys));
    }
    let mut labels: HashMap<_, _> = keys
        .iter()
        .enumerate()
        .map(|(label, id)| (*id, label as u64))
        .collect();
    for id in ids {
        if let Some(label) = labels.get(&id) {
            index.remove(*label).internal()?;
        }
    }
    index
        .reserve(index.size().saturating_add(rows.len()))
        .internal()?;
    for row in rows {
        let vector = row
            .embedding
            .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("missing index vector")))?;
        super::validate_vector(vector.as_slice(), dimensions)?;
        let label = *labels.entry(row.entity_id).or_insert_with(|| {
            let label = keys.len() as u64;
            keys.push(row.entity_id);
            label
        });
        index
            .add(label, &normalized(vector.as_slice()))
            .internal()?;
    }
    Ok((index, keys))
}

pub(super) async fn maintain(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    dimensions: usize,
) -> Result<(), YorishiroError> {
    // Read revision and vectors from one MVCC snapshot; publication rejects any intervening write.
    let txn = db
        .begin_with_config(
            Some(sea_orm::IsolationLevel::RepeatableRead),
            Some(sea_orm::AccessMode::ReadOnly),
        )
        .await
        .internal()?;
    let state = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT revision, dimensions, format, pending FROM entity_embedding_indexes WHERE workspace_id=$1",
            [workspace_id.into()],
        ))
        .await
        .internal()?;
    let Some(state) = state else {
        txn.rollback().await.internal()?;
        return Ok(());
    };
    let revision: i64 = state.try_get("", "revision").internal()?;
    let compatible = state.try_get::<Option<i32>>("", "dimensions").internal()?
        == Some(dimensions as i32)
        && state
            .try_get::<Option<String>>("", "format")
            .internal()?
            .as_deref()
            == Some(FORMAT);
    let pending: serde_json::Value = state.try_get("", "pending").internal()?;
    if compatible && pending.as_object().is_some_and(|ids| ids.is_empty()) {
        txn.rollback().await.internal()?;
        return Ok(());
    }
    // Fetch large graph blobs only when an existing index actually needs incremental maintenance.
    let previous = if compatible {
        txn.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT graph, vectors FROM entity_embedding_indexes WHERE workspace_id=$1",
            [workspace_id.into()],
        ))
        .await
        .internal()?
    } else {
        None
    };
    let old_graph: Option<Vec<u8>> = previous
        .as_ref()
        .map(|state| state.try_get("", "graph").internal())
        .transpose()?
        .flatten();
    let old_vectors: Option<Vec<u8>> = previous
        .as_ref()
        .map(|state| state.try_get("", "vectors").internal())
        .transpose()?
        .flatten();
    let delta = if compatible {
        Some(changes(&txn, workspace_id, pending).await?)
    } else {
        None
    };
    let rows = if delta.is_none() {
        entity_embeddings::Entity::find()
            .filter(entity_embeddings::Column::WorkspaceId.eq(workspace_id))
            .filter(entity_embeddings::Column::Embedding.is_not_null())
            .order_by_asc(entity_embeddings::Column::EntityId)
            .all(&txn)
            .await
            .internal()?
    } else {
        Vec::new()
    };
    txn.rollback().await.internal()?;
    if !compatible && rows.len() < 2000 {
        return Ok(());
    }
    if rows.len() > u32::MAX as usize {
        return Err(YorishiroError::BackendUnavailable {
            message: "workspace exceeds the HNSW node limit".into(),
        });
    }
    let (graph, vectors) = tokio::task::spawn_blocking(move || {
        if let (Some(graph), Some(vectors), Some((ids, changed))) = (old_graph, old_vectors, delta)
        {
            let (graph, keys) = load(&graph, vectors, dimensions, ids, changed)?;
            return save(&graph, &keys);
        }
        build(rows, dimensions)
    })
    .await
    .internal()??;
    // Atomic compare-and-publish needs one statement; ordinary updates could publish a stale graph.
    let published = db.execute_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "UPDATE entity_embedding_indexes i SET dimensions=$1, graph=$2, vectors=$3, format=$6, pending='{}'::jsonb WHERE workspace_id=$4 AND revision=$5 AND EXISTS (SELECT 1 FROM workspace_workspaces w WHERE w.id=i.workspace_id AND NOT w.embedding_reindexing AND w.embedding_dimensions=$1)",
        [(dimensions as i32).into(), graph.into(), vectors.into(), workspace_id.into(), revision.into(), FORMAT.into()]))
        .await.internal()?;
    tracing::debug!(%workspace_id, dimensions, format = FORMAT, published = published.rows_affected() == 1, "HNSW maintenance finished");
    Ok(())
}

pub(crate) async fn candidates(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    vector: &[f32],
    allowed: Option<std::collections::HashSet<Uuid>>,
) -> Result<Option<Vec<Uuid>>, YorishiroError> {
    // A shared advisory lock avoids granting tenant callers UPDATE merely to use FOR SHARE.
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock_shared(hashtextextended('embedding-hnsw:' || $1::uuid::text, 0))",
        [workspace_id.into()],
    ))
    .await
    .internal()?;
    let state = conn.query_one_raw(Statement::from_sql_and_values(DatabaseBackend::Postgres,
        "SELECT graph, vectors, pending FROM entity_embedding_indexes WHERE workspace_id=$1 AND dimensions=$2 AND format=$3",
        [workspace_id.into(), (vector.len() as i32).into(), FORMAT.into()])).await.internal()?;
    let Some(state) = state else {
        return Ok(None);
    };
    let graph: Option<Vec<u8>> = state.try_get("", "graph").internal()?;
    let vectors: Option<Vec<u8>> = state.try_get("", "vectors").internal()?;
    let (Some(graph), Some(vectors)) = (graph, vectors) else {
        return Ok(None);
    };
    let dimensions = vector.len();
    let pending = state.try_get("", "pending").internal()?;
    let (ids, rows) = changes(conn, workspace_id, pending).await?;
    let query = normalized(vector);
    // shortcut: deserialize each search; add a revision-keyed bounded resident index when measured reload cost exceeds the search budget.
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let serialized = graph;
        let (graph, keys) = if ids.is_empty() && rows.is_empty() {
            let index = new_index(dimensions)?;
            // SAFETY: serialized is immutable and outlives index, which never leaves this closure.
            unsafe { index.view_from_buffer(&serialized) }.internal()?;
            index.change_expansion_search(2000);
            (index, serde_json::from_slice::<Vec<Uuid>>(&vectors).internal()?)
        } else {
            load(&serialized, vectors, dimensions, ids, rows)?
        };
        let load_ms = started.elapsed().as_secs_f64() * 1000.0;
        if graph.size() == 0 {
            return Ok(Some(Vec::new()));
        }
        let filter = |label: u64| {
            keys.get(label as usize)
                .is_some_and(|id| allowed.as_ref().is_none_or(|ids| ids.contains(id)))
        };
        let hits = graph
            .filtered_search(&query, CANDIDATE_LIMIT, filter)
            .internal()?;
        tracing::debug!(%workspace_id, dimensions, nodes = graph.size(), candidates = hits.keys.len(), load_ms, total_ms = started.elapsed().as_secs_f64() * 1000.0, "HNSW candidates selected");
        let ids = hits
            .keys
            .into_iter()
            .map(|label| {
                keys.get(label as usize)
                    .copied()
                    .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("invalid HNSW label")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(ids))
    })
    .await
    .internal()?
}
