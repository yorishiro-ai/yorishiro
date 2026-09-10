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
//! cargo loco task sqlite_ann_benchmark \
//!     workspace_id:<uuid> \
//!     [min_entities:<int>] \
//!     [max_latency_ms:<int>]
//! ```

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities;
use crate::models::search::SearchHit;
use crate::models::search::SearchRow;
use crate::models::search::resolve_search_table;
use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::EntityTrait;
use sea_orm::FromQueryResult;
use sea_orm::PaginatorTrait;
use sea_orm::Statement;
use uuid::Uuid;

/// `cargo loco task sqlite_ann_benchmark workspace_id:<uuid> [min_entities:<int>] [max_latency_ms:<int>]`
///
/// Measures full-scan vector search latency on SQLite and reports whether
/// the vec0 virtual table should be adopted.
pub struct SqliteAnnBenchmark;

#[async_trait]
impl Task for SqliteAnnBenchmark {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "sqlite_ann_benchmark".to_string(),
            detail: "Measures SQLite full-scan vector search latency and recommends vec0 adoption if thresholds exceeded: cargo loco task sqlite_ann_benchmark workspace_id:<uuid> [min_entities:<int>] [max_latency_ms:<int>]".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        // Only meaningful on SQLite.
        if ctx.db.get_database_backend() != sea_orm::DatabaseBackend::Sqlite {
            return Err(Error::Message(
                "sqlite_ann_benchmark is only meaningful on SQLite".to_string(),
            ));
        }

        let workspace_id: Uuid = vars
            .cli_arg("workspace_id")
            .map_err(|_| Error::Message("workspace_id is required".to_string()))?
            .parse()
            .map_err(|_| Error::Message("workspace_id is not a valid UUID".to_string()))?;

        let min_entities: usize = match vars.cli_arg("min_entities") {
            Ok(raw) => raw
                .parse::<usize>()
                .map_err(|_| Error::Message("min_entities is not a valid integer".to_string()))?,
            Err(_) => 1000,
        };

        let max_latency_ms: u64 = match vars.cli_arg("max_latency_ms") {
            Ok(raw) => raw
                .parse::<u64>()
                .map_err(|_| Error::Message("max_latency_ms is not a valid integer".to_string()))?,
            Err(_) => 200,
        };

        // Resolve the correct width-partitioned table for this workspace.
        let licenced = ctx
            .shared_store
            .get::<crate::ee::services::licence::LicenceState>()
            .is_some_and(|state| state.is_active());
        let (_dimension, table_name) = resolve_search_table(&ctx.db, workspace_id, licenced)
            .await
            .map_err(|e| Error::Message(e.to_string()))?;

        // Count entities with embeddings in this workspace.
        let entity_count = entity_entities::Entity::find()
            .count(&ctx.db)
            .await
            .internal()
            .map_err(|e| Error::Message(e.to_string()))?;

        let entity_count = entity_count as usize;

        // Need at least one embedding to run the benchmark.
        if entity_count == 0 {
            println!("no embeddings found for workspace {workspace_id}: nothing to benchmark");
            return Ok(());
        }

        // Fetch one entity that has an embedding to extract its vector.
        let sample_row = fetch_sample_embedding(&ctx.db, workspace_id, &table_name)
            .await
            .map_err(|e| Error::Message(e.to_string()))?;

        match sample_row {
            Some((_, vector_blob)) => {
                let vector = vector_blob_to_f32(vector_blob);
                run_benchmark(
                    ctx,
                    workspace_id,
                    &vector,
                    &table_name,
                    entity_count,
                    min_entities,
                    max_latency_ms,
                )
                .await
            }
            None => {
                println!("no embeddings with vector data found for workspace {workspace_id}");
                Ok(())
            }
        }
    }
}

/// Row returned from the embedding table.
#[derive(sea_orm::FromQueryResult)]
struct EmbeddingRow {
    entity_id: Uuid,
    embedding: Vec<u8>,
}

async fn fetch_sample_embedding(
    conn: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    table_name: &str,
) -> std::result::Result<Option<(Uuid, Vec<u8>)>, YorishiroError> {
    let sql = format!(
        "SELECT ee.entity_id, ee.embedding FROM {table_name} ee \
         JOIN entity_entities e ON e.id = ee.entity_id \
         WHERE e.workspace_id = ? \
         LIMIT 1"
    );

    let row = EmbeddingRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        &sql,
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await
    .internal()?;

    Ok(row.map(|er| (er.entity_id, er.embedding)))
}

fn vector_blob_to_f32(blob: Vec<u8>) -> Vec<f32> {
    // Re-interpret the BLOB as f32 LE values.
    assert_eq!(
        blob.len() % 4,
        0,
        "embedding BLOB not a multiple of 4 bytes"
    );
    let len = blob.len() / 4;
    let mut vec = vec![0.0_f32; len];
    unsafe {
        std::ptr::copy_nonoverlapping(blob.as_ptr(), vec.as_mut_ptr() as *mut u8, blob.len());
    }
    vec
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
    ctx: &AppContext,
    workspace_id: Uuid,
    vector: &[f32],
    table_name: &str,
    entity_count: usize,
    min_entities: usize,
    max_latency_ms: u64,
) -> Result<()> {
    let iterations = 5;

    // Phase 1: full-scan benchmark.
    let full_scan = benchmark_phase(&ctx.db, workspace_id, vector, table_name, iterations, true)
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
    let vec0_result = check_vec0_availability(&ctx.db, table_name)
        .await
        .map_err(|e| Error::Message(e.to_string()))?;

    if vec0_result.available {
        let vec0 = benchmark_phase(&ctx.db, workspace_id, vector, table_name, iterations, false)
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
        println!("  speedup vs full-scan: {:.1}x", speedup);
    } else {
        println!();
        println!("--- vec0: not available on this deployment ---");
    }

    // Recommendation.
    println!();
    println!("--- Recommendation ---");
    let entities_ok = entity_count >= min_entities;
    let latency_ok = full_scan.median_ms <= max_latency_ms as f64;

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
            let vec0 =
                benchmark_phase(&ctx.db, workspace_id, vector, table_name, iterations, false)
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
    conn: &sea_orm::DatabaseConnection,
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
        let _hits: Vec<SearchHit> = SearchRow::find_by_statement(Statement::from_sql_and_values(
            conn.get_database_backend(),
            &knn.sql,
            knn.values.clone(),
        ))
        .all(conn)
        .await
        .internal()?
        .into_iter()
        .map(SearchRow::into_hit)
        .collect();
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = latencies.len() / 2;
    let median = if latencies.len() % 2 == 0 {
        (latencies[mid - 1] + latencies[mid]) / 2.0
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

/// SQL query for full-scan vector search (uses `vec_distance_cosine`).
struct KnnQuery {
    sql: String,
    values: Vec<sea_orm::Value>,
}

fn make_full_scan_query(vector: &[f32], workspace_id: Uuid, table_name: &str) -> KnnQuery {
    // Convert the vector to raw LE f32 bytes for `vec_distance_cosine`.
    let blob_bytes =
        unsafe { std::slice::from_raw_parts(vector.as_ptr() as *const u8, vector.len() * 4) };

    let sql = format!(
        "SELECT e.id, e.workspace_id, e.schema_id, e.schema_version, \
         e.entity_type, e.data, e.created_at, e.updated_at, \
         e.created_by, e.updated_by, \
         vec_distance_cosine(?, ee.embedding) AS distance \
         FROM entity_entities e \
         JOIN {table_name} ee ON e.id = ee.entity_id \
         WHERE ee.embedding IS NOT NULL AND e.workspace_id = ? \
         ORDER BY distance LIMIT 10"
    );
    let values = vec![
        sea_orm::Value::from(blob_bytes.to_vec()),
        workspace_id.into(),
    ];
    KnnQuery { sql, values }
}

/// SQL query for vec0 KNN search (uses the vec0 virtual table).
///
/// The vec0 virtual table is created by sqlite-vec as `{table_name}_vec0`
/// and exposes a `MATCH k=` operator for KNN search.
fn make_vec0_query(vector: &[f32], workspace_id: Uuid, table_name: &str) -> KnnQuery {
    // Convert the vector to raw LE f32 bytes for vec0.
    let blob_bytes =
        unsafe { std::slice::from_raw_parts(vector.as_ptr() as *const u8, vector.len() * 4) };

    let sql = format!(
        "SELECT e.id, e.workspace_id, e.schema_id, e.schema_version, \
         e.entity_type, e.data, e.created_at, e.updated_at, \
         e.created_by, e.updated_by, \
         ee.distance AS distance \
         FROM entity_entities e \
         JOIN {table_name}_vec0 ee \
             ON ee.rowid = e.id \
             AND ee MATCH (k=10) \
         WHERE e.workspace_id = ? \
         ORDER BY ee.distance LIMIT 10"
    );
    let values = vec![
        sea_orm::Value::from(blob_bytes.to_vec()),
        workspace_id.into(),
    ];
    KnnQuery { sql, values }
}

/// Check whether the vec0 virtual table exists for the given embedding table.
///
/// Returns `PhaseResult { available: true }` if the virtual table is found,
/// `PhaseResult { available: false }` if it is not.
async fn check_vec0_availability(
    conn: &sea_orm::DatabaseConnection,
    table_name: &str,
) -> std::result::Result<PhaseResult, YorishiroError> {
    let vec0_table = format!("{table_name}_vec0");

    // SQLite stores virtual tables in sqlite_master like regular tables.
    let sql = "SELECT 1 FROM sqlite_master WHERE name = ? LIMIT 1";

    #[derive(sea_orm::FromQueryResult)]
    struct MasterRow {
        #[sea_orm(column_name = "type")]
        _typ: String,
    }

    let row = MasterRow::find_by_statement(Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        vec![vec0_table.clone().into()],
    ))
    .one(conn)
    .await
    .internal()?;

    Ok(PhaseResult {
        median_ms: 0.0,
        max_ms: 0.0,
        min_ms: 0.0,
        available: row.is_some(),
    })
}
