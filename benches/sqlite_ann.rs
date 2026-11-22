//! Benchmarks SQLite full-scan vector search and compares it against vec0 if available.
//!
//! This is a measurement task, not a migration or adoption tool: it reports
//! whether a deployment's entity count and scan latency justify moving to the
//! vec0 virtual table (KNN index). It does not change any schema or configuration.
//!
//! # Thresholds
//!
//! The task uses two thresholds:
//! - `min_entities` (default 1000): fewer entities than this makes vec0
//!   unnecessary regardless of latency.
//! - `max_latency_ms` (default 200): full-scan latencies below this are
//!   acceptable on the current entity count.
//!
//! When both thresholds are exceeded (entity count >= min AND latency > max),
//! the report recommends adopting vec0. Below, it reports that full-scan is
//! still acceptable.
//!
//! # Usage
//!
//! ```sh
//! cargo bench --bench sqlite_ann -- sqlite:///path/to/database.sqlite3 WORKSPACE_UUID
//! ```

use loco_rs::prelude::{Error, Result};
use sea_orm::FromQueryResult;
use sea_orm::Statement;
use sea_orm::sea_query::{Alias, Asterisk, Expr, ExprTrait, JoinType, Query};
use sea_orm::sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect, RelationTrait};
use sea_orm::{ConnectionTrait, DatabaseConnection, SqlxSqliteConnector};
use uuid::Uuid;
use yorishiro::error::{ResultExt, YorishiroError};
use yorishiro::models::_entities::entity_embeddings;
use yorishiro::models::_entities::entity_entities::Column as EntityColumn;
use yorishiro::models::entity_entities;

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
    let table_name = "entity_embeddings";

    // Count entities with embeddings in this workspace.
    let entity_count = entity_embeddings::Entity::find()
        .join(
            JoinType::InnerJoin,
            entity_embeddings::Relation::EntityEntities.def(),
        )
        .filter(EntityColumn::WorkspaceId.eq(workspace_id))
        .filter(entity_embeddings::Column::Embedding.is_not_null())
        .count(&db)
        .await?;
    let entity_count = usize::try_from(entity_count)
        .map_err(|_| Error::Message("embedding count exceeds platform limits".into()))?;

    // Need at least one embedding to run the benchmark.
    if entity_count == 0 {
        println!("no embeddings found for workspace {workspace_id}: nothing to benchmark");
        return Ok(());
    }

    // Fetch one entity that has an embedding to extract its vector.
    let sample_row = fetch_sample_embedding(&db, workspace_id, table_name)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    if let Some((_, vector_blob)) = sample_row {
        let vector = vector_blob_to_f32(&vector_blob);
        run_benchmark(
            &db,
            workspace_id,
            &vector,
            table_name,
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

/// Row returned from the embedding table.
#[derive(sea_orm::FromQueryResult)]
struct EmbeddingRow {
    entity_id: Uuid,
    embedding: Vec<u8>,
}

async fn fetch_sample_embedding(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    table_name: &str,
) -> std::result::Result<Option<(Uuid, Vec<u8>)>, YorishiroError> {
    let query = Query::select()
        .columns([
            (Alias::new("ee"), Alias::new("entity_id")),
            (Alias::new("ee"), Alias::new("embedding")),
        ])
        .from_as(Alias::new(table_name), Alias::new("ee"))
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

    Ok(row.map(|er| (er.entity_id, er.embedding)))
}

fn vector_blob_to_f32(blob: &[u8]) -> Vec<f32> {
    // Re-interpret the BLOB as f32 LE values.
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

/// Results from one measurement phase.
struct PhaseResult {
    /// Median latency in milliseconds across iterations.
    median_ms: f64,
    /// Max latency across iterations.
    max_ms: f64,
    /// Min latency across iterations.
    min_ms: f64,
    /// Whether this method is available on this deployment.
    available: bool,
}

async fn run_benchmark(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    vector: &[f32],
    table_name: &str,
    entity_count: usize,
    min_entities: usize,
    max_latency_ms: u64,
) -> Result<()> {
    let iterations = 5;

    // Phase 1: full-scan benchmark.
    let full_scan = benchmark_phase(db, workspace_id, vector, table_name, iterations, true)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    println!("=== SQLite ANN Benchmark ===");
    println!("workspace_id: {workspace_id}");
    println!("embedding width: {}", vector.len());
    println!("table: {table_name}");
    println!("entities with embeddings: {entity_count}");
    println!("iterations: {iterations}");
    println!();
    println!("--- Full-scan (vec_distance_cosine) ---");
    println!("  median: {:.0} ms", full_scan.median_ms);
    println!("  min:    {:.0} ms", full_scan.min_ms);
    println!("  max:    {:.0} ms", full_scan.max_ms);

    // Phase 2: vec0 benchmark (if the virtual table exists).
    let vec0_result = check_vec0_availability(db, table_name)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    if vec0_result.available {
        let vec0 = benchmark_phase(db, workspace_id, vector, table_name, iterations, false)
            .await
            .map_err(|e| Error::Message(e.to_string()))?;

        println!();
        println!("--- vec0 (KNN virtual table) ---");
        println!("  median: {:.0} ms", vec0.median_ms);
        println!("  min:    {:.0} ms", vec0.min_ms);
        println!("  max:    {:.0} ms", vec0.max_ms);

        let speedup = if vec0.median_ms > 0.0 {
            full_scan.median_ms / vec0.median_ms
        } else {
            f64::INFINITY
        };
        println!("  speedup vs full-scan: {speedup:.1}x");
    } else {
        println!();
        println!("--- vec0: not available on this deployment ---");
    }

    // Recommendation.
    println!();
    println!("--- Recommendation ---");
    let entities_ok = entity_count >= min_entities;
    let latency_ok = full_scan.median_ms
        <= std::time::Duration::from_millis(max_latency_ms).as_secs_f64() * 1000.0;

    if !entities_ok {
        println!(
            "entity count ({entity_count}) is below the threshold ({min_entities}). \
             Full-scan is fine for this size."
        );
    } else if latency_ok {
        println!(
            "median latency ({:.0} ms) is below the threshold ({} ms). \
             Full-scan is acceptable for {} entities.",
            full_scan.median_ms, max_latency_ms, entity_count
        );
    } else {
        println!(
            "median latency ({:.0} ms) exceeds the threshold ({} ms) \
             with {} entities. Consider adopting the vec0 virtual table for KNN search.",
            full_scan.median_ms, max_latency_ms, entity_count
        );
        println!(
            "  vec0 was {}",
            if vec0_result.available {
                "available"
            } else {
                "not available"
            }
        );
        if vec0_result.available {
            let vec0 = benchmark_phase(db, workspace_id, vector, table_name, iterations, false)
                .await
                .map_err(|e| Error::Message(e.to_string()))?;
            println!(
                "  vec0 median latency: {:.0} ms ({:.1}x faster)",
                vec0.median_ms,
                if vec0.median_ms > 0.0 {
                    full_scan.median_ms / vec0.median_ms
                } else {
                    f64::INFINITY
                }
            );
        }
    }

    Ok(())
}

async fn benchmark_phase(
    conn: &DatabaseConnection,
    workspace_id: Uuid,
    vector: &[f32],
    table_name: &str,
    iterations: usize,
    use_full_scan: bool,
) -> std::result::Result<PhaseResult, YorishiroError> {
    let knn = if use_full_scan {
        make_full_scan_query(vector, workspace_id, table_name)
    } else {
        make_vec0_query(vector, workspace_id, table_name)
    };

    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        let hits = conn.query_all_raw(knn.clone()).await.internal()?;
        if hits.is_empty() {
            return Err(YorishiroError::Internal(
                "benchmark query returned no entities for a workspace with a sample embedding"
                    .into(),
            ));
        }
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = latencies.len() / 2;
    let median = if latencies.len() % 2 == 0 {
        f64::midpoint(latencies[mid - 1], latencies[mid])
    } else {
        latencies[mid]
    };

    Ok(PhaseResult {
        median_ms: median,
        max_ms: *latencies.last().unwrap(),
        min_ms: latencies[0],
        available: true,
    })
}

fn make_full_scan_query(vector: &[f32], workspace_id: Uuid, table_name: &str) -> Statement {
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
            Alias::new(table_name),
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
/// The optional `{table_name}_vec0` table uses canonical UUID TEXT `entity_id`
/// as its primary key, a TEXT `workspace_id` partition key, and cosine `embedding` vectors.
fn make_vec0_query(vector: &[f32], workspace_id: Uuid, table_name: &str) -> Statement {
    use sea_orm::sea_query::{Func, Order, extension::sqlite::SqliteBinOper};
    let knn = Query::select()
        .columns(["entity_id", "distance"].map(Alias::new))
        .from(Alias::new(format!("{table_name}_vec0")))
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

/// Check whether the vec0 virtual table exists for the given embedding table.
///
/// Returns `PhaseResult { available: true }` if the virtual table is found,
/// `PhaseResult { available: false }` if it is not.
async fn check_vec0_availability(
    conn: &DatabaseConnection,
    table_name: &str,
) -> std::result::Result<PhaseResult, YorishiroError> {
    let vec0_table = format!("{table_name}_vec0");

    let query = Query::select()
        .column(Asterisk)
        .from(Alias::new("sqlite_master"))
        .and_where(Expr::col(Alias::new("name")).eq(vec0_table))
        .limit(1)
        .to_owned();
    let row = conn
        .query_one_raw(conn.get_database_backend().build(&query))
        .await
        .internal()?;

    Ok(PhaseResult {
        median_ms: 0.0,
        max_ms: 0.0,
        min_ms: 0.0,
        available: row.is_some(),
    })
}
