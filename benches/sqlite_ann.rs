//! Benchmarks SQLite full-scan vector search and compares it against vec0 if available.
//!
//! The database is opened read-only.
//!
//! # Usage
//!
//! ```sh
//! cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID
//! ```

use loco_rs::prelude::{Error, Result};
use sea_orm::FromQueryResult;
use sea_orm::Statement;
use sea_orm::sea_query::{Alias, Expr, ExprTrait, JoinType, Query};
use sea_orm::sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect, RelationTrait};
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlxSqliteConnector};
use uuid::Uuid;
use yorishiro::error::{ResultExt, YorishiroError};
use yorishiro::models::_entities::entity_embeddings;
use yorishiro::models::_entities::entity_entities::Column as EntityColumn;
use yorishiro::models::entity_entities;

const EMBEDDINGS: &str = "entity_embeddings";
const VEC0: &str = "entity_embeddings_vec0";
const ITERATIONS: usize = 5;

#[tokio::main]
async fn main() -> Result<()> {
    let usage = "usage: cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID [MIN_ENTITIES] [MAX_LATENCY_MS]";
    let args: Vec<_> = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--bench")
        .collect();
    let [url, workspace_id, thresholds @ ..] = args.as_slice() else {
        println!("{usage}");
        return Ok(());
    };
    if !url.starts_with("sqlite:") || thresholds.len() > 2 {
        return Err(Error::Message(usage.into()));
    }
    let workspace_id = workspace_id
        .parse()
        .map_err(|_| Error::Message(usage.into()))?;
    let min_entities = thresholds
        .first()
        .map_or(Ok(1000), |arg| arg.parse())
        .map_err(|_| Error::Message(usage.into()))?;
    let max_latency_ms = thresholds
        .get(1)
        .map_or(Ok(200), |arg| arg.parse())
        .map_err(|_| Error::Message(usage.into()))?;
    yorishiro::db::register_sqlite_extensions();
    let options = url
        .parse::<SqliteConnectOptions>()
        .map_err(|e| Error::Message(e.to_string()))?
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;
    let db = SqlxSqliteConnector::from_sqlx_sqlite_pool(pool);

    let entity_count = entity_embeddings::Entity::find()
        .join(
            JoinType::InnerJoin,
            entity_embeddings::Relation::EntityEntities.def(),
        )
        .filter(EntityColumn::WorkspaceId.eq(workspace_id))
        .filter(entity_embeddings::Column::Embedding.is_not_null())
        .count(&db)
        .await?;

    if entity_count == 0 {
        println!("no embeddings found for workspace {workspace_id}: nothing to benchmark");
        return Ok(());
    }

    let sample_row = fetch_sample_embedding(&db, workspace_id)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    if let Some(vector_blob) = sample_row {
        let vector = vector_blob_to_f32(&vector_blob);
        run_benchmark(
            &db,
            workspace_id,
            &vector,
            entity_count,
            min_entities,
            max_latency_ms,
        )
        .await
    } else {
        println!("no embeddings with vector data found for workspace {workspace_id}");
        Ok(())
    }
}

#[derive(sea_orm::FromQueryResult)]
struct EmbeddingRow {
    embedding: Vec<u8>,
}

async fn fetch_sample_embedding(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
) -> std::result::Result<Option<Vec<u8>>, YorishiroError> {
    let query = Query::select()
        .column((Alias::new("ee"), Alias::new("embedding")))
        .from_as(Alias::new(EMBEDDINGS), Alias::new("ee"))
        .join_as(
            JoinType::InnerJoin,
            entity_entities::Entity,
            Alias::new("e"),
            Expr::col((Alias::new("e"), EntityColumn::Id))
                .equals((Alias::new("ee"), Alias::new("entity_id"))),
        )
        .and_where(Expr::col((Alias::new("e"), EntityColumn::WorkspaceId)).eq(workspace_id))
        .and_where(Expr::col((Alias::new("ee"), Alias::new("embedding"))).is_not_null())
        .limit(1)
        .to_owned();
    let row = EmbeddingRow::find_by_statement(conn.get_database_backend().build(&query))
        .one(conn)
        .await
        .internal()?;

    Ok(row.map(|row| row.embedding))
}

fn vector_blob_to_f32(blob: &[u8]) -> Vec<f32> {
    assert_eq!(
        blob.len() % 4,
        0,
        "embedding BLOB not a multiple of 4 bytes"
    );
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

struct LatencyMs {
    median: f64,
    max: f64,
    min: f64,
}

async fn run_benchmark(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    vector: &[f32],
    entity_count: u64,
    min_entities: u64,
    max_latency_ms: u64,
) -> Result<()> {
    let full_scan = benchmark_phase(db, make_full_scan_query(vector, workspace_id))
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    println!("=== SQLite ANN Benchmark ===");
    println!("workspace_id: {workspace_id}");
    println!("embedding width: {}", vector.len());
    println!("entities with embeddings: {entity_count}");
    println!("iterations: {ITERATIONS}");
    println!();
    println!("--- Full-scan (vec_distance_cosine) ---");
    println!("  median: {:.0} ms", full_scan.median);
    println!("  min:    {:.0} ms", full_scan.min);
    println!("  max:    {:.0} ms", full_scan.max);

    if vec0_available(db)
        .await
        .map_err(|e| Error::Message(e.to_string()))?
    {
        let vec0 = benchmark_phase(db, make_vec0_query(vector, workspace_id))
            .await
            .map_err(|e| Error::Message(e.to_string()))?;

        println!();
        println!("--- vec0 (KNN virtual table) ---");
        println!("  median: {:.0} ms", vec0.median);
        println!("  min:    {:.0} ms", vec0.min);
        println!("  max:    {:.0} ms", vec0.max);

        let speedup = full_scan.median / vec0.median;
        println!("  speedup vs full-scan: {speedup:.1}x");
    } else {
        println!();
        println!("--- vec0: not available on this deployment ---");
    }

    println!();
    println!("--- Recommendation ---");
    let entities_ok = entity_count >= min_entities;
    let latency_ok =
        full_scan.median <= std::time::Duration::from_millis(max_latency_ms).as_secs_f64() * 1000.0;

    if !entities_ok {
        println!(
            "entity count ({entity_count}) is below the threshold ({min_entities}). \
             Full-scan is fine for this size."
        );
    } else if latency_ok {
        println!(
            "median latency ({:.0} ms) is below the threshold ({} ms). \
             Full-scan is acceptable for {} entities.",
            full_scan.median, max_latency_ms, entity_count
        );
    } else {
        println!(
            "median latency ({:.0} ms) exceeds the threshold ({} ms) \
             with {} entities. Consider adopting the vec0 virtual table for KNN search.",
            full_scan.median, max_latency_ms, entity_count
        );
    }

    Ok(())
}

async fn benchmark_phase(
    conn: &DatabaseConnection,
    query: Statement,
) -> std::result::Result<LatencyMs, YorishiroError> {
    let mut latencies = [0.0; ITERATIONS];
    for latency in &mut latencies {
        let start = std::time::Instant::now();
        let hits = conn.query_all_raw(query.clone()).await.internal()?;
        if hits.is_empty() {
            return Err(YorishiroError::Internal(
                "benchmark query returned no entities for a workspace with a sample embedding"
                    .into(),
            ));
        }
        *latency = start.elapsed().as_secs_f64() * 1000.0;
    }

    latencies.sort_by(f64::total_cmp);

    Ok(LatencyMs {
        median: latencies[ITERATIONS / 2],
        max: latencies[ITERATIONS - 1],
        min: latencies[0],
    })
}

fn make_full_scan_query(vector: &[f32], workspace_id: Uuid) -> Statement {
    use sea_orm::sea_query::{Func, Order};
    let query = Query::select()
        .columns(
            [
                EntityColumn::Id,
                EntityColumn::WorkspaceId,
                EntityColumn::SchemaId,
                EntityColumn::SchemaVersion,
                EntityColumn::EntityType,
                EntityColumn::Data,
                EntityColumn::CreatedAt,
                EntityColumn::UpdatedAt,
                EntityColumn::CreatedBy,
                EntityColumn::UpdatedBy,
            ]
            .map(|column| (Alias::new("e"), column)),
        )
        .expr_as(
            Func::cust(Alias::new("vec_distance_cosine")).args([
                Expr::val(yorishiro::db::sqlite_vec_blob(vector)),
                Expr::col((Alias::new("ee"), Alias::new("embedding"))),
            ]),
            "distance",
        )
        .from_as(entity_entities::Entity, Alias::new("e"))
        .join_as(
            JoinType::InnerJoin,
            Alias::new(EMBEDDINGS),
            Alias::new("ee"),
            Expr::col((Alias::new("e"), EntityColumn::Id))
                .equals((Alias::new("ee"), Alias::new("entity_id"))),
        )
        .and_where(Expr::col((Alias::new("ee"), Alias::new("embedding"))).is_not_null())
        .and_where(Expr::col((Alias::new("e"), EntityColumn::WorkspaceId)).eq(workspace_id))
        .order_by(Alias::new("distance"), Order::Asc)
        .limit(10)
        .to_owned();
    sea_orm::DatabaseBackend::Sqlite.build(&query)
}

/// SQL query for vec0 KNN search (uses the vec0 virtual table).
///
/// The optional `entity_embeddings_vec0` table uses canonical UUID TEXT `entity_id`
/// as its primary key, a TEXT `workspace_id` partition key, and cosine `embedding` vectors.
fn make_vec0_query(vector: &[f32], workspace_id: Uuid) -> Statement {
    use sea_orm::sea_query::{Func, Order, extension::sqlite::SqliteBinOper};
    let knn = Query::select()
        .columns(["entity_id", "distance"].map(Alias::new))
        .from(Alias::new(VEC0))
        .and_where(Expr::col(Alias::new("embedding")).binary(
            SqliteBinOper::Match,
            Expr::val(yorishiro::db::sqlite_vec_blob(vector)),
        ))
        .and_where(Expr::col(Alias::new("k")).eq(10))
        .and_where(Expr::col(Alias::new("workspace_id")).eq(workspace_id.to_string()))
        .to_owned();
    let query = Query::select()
        .columns(
            [
                EntityColumn::Id,
                EntityColumn::WorkspaceId,
                EntityColumn::SchemaId,
                EntityColumn::SchemaVersion,
                EntityColumn::EntityType,
                EntityColumn::Data,
                EntityColumn::CreatedAt,
                EntityColumn::UpdatedAt,
                EntityColumn::CreatedBy,
                EntityColumn::UpdatedBy,
            ]
            .map(|column| (Alias::new("e"), column)),
        )
        .column((Alias::new("ee"), Alias::new("distance")))
        .from_subquery(knn, Alias::new("ee"))
        .join_as(
            JoinType::InnerJoin,
            entity_entities::Entity,
            Alias::new("e"),
            Func::cust(Alias::new("replace"))
                .args([
                    Expr::col((Alias::new("ee"), Alias::new("entity_id"))),
                    Expr::val("-"),
                    Expr::val(""),
                ])
                .eq(Func::lower(
                    Func::cust(Alias::new("hex"))
                        .arg(Expr::col((Alias::new("e"), EntityColumn::Id))),
                )),
        )
        .and_where(Expr::col((Alias::new("e"), EntityColumn::WorkspaceId)).eq(workspace_id))
        .order_by((Alias::new("ee"), Alias::new("distance")), Order::Asc)
        .limit(10)
        .to_owned();
    sea_orm::DatabaseBackend::Sqlite.build(&query)
}

async fn vec0_available(conn: &DatabaseConnection) -> std::result::Result<bool, YorishiroError> {
    let query = Query::select()
        .column(Alias::new("name"))
        .from(Alias::new("sqlite_master"))
        .and_where(Expr::col(Alias::new("name")).eq(VEC0))
        .limit(1)
        .to_owned();
    let row = conn
        .query_one_raw(conn.get_database_backend().build(&query))
        .await
        .internal()?;

    Ok(row.is_some())
}
