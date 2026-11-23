//! Measures PostgreSQL vector search against an existing workspace in a read-only transaction.
//!
//! ```sh
//! cargo bench --bench postgres_search -- DATABASE_URL WORKSPACE_UUID QUERY_VECTOR_JSON
//! ```

use std::time::Instant;

use sea_orm::{ConnectionTrait, Database, TransactionTrait};
use uuid::Uuid;
use yorishiro::models::search::{SearchQuery, search_by_vector};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--bench")
        .collect();
    let [url, workspace, vector_file] = args.as_slice() else {
        println!(
            "usage: cargo bench --bench postgres_search -- DATABASE_URL WORKSPACE_UUID QUERY_VECTOR_JSON"
        );
        return Ok(());
    };
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        return Err("PostgreSQL URL required".into());
    }
    let workspace: Uuid = workspace.parse()?;
    let vector: Vec<f32> = serde_json::from_slice(&std::fs::read(vector_file)?)?;
    if vector.is_empty() || !vector.iter().all(|value| value.is_finite()) {
        return Err("query vector must contain finite coordinates".into());
    }
    let db = Database::connect(url).await?;
    let txn = db.begin().await?;
    txn.execute_unprepared("SET TRANSACTION READ ONLY").await?;
    let mut milliseconds = Vec::with_capacity(5);
    for _ in 0..5 {
        let query = SearchQuery {
            limit: 10,
            ..Default::default()
        };
        let started = Instant::now();
        let hits = search_by_vector(&txn, workspace, vector.clone(), "", query).await?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if hits.is_empty() {
            return Err(
                "search returned no entities; check workspace access and embeddings".into(),
            );
        }
        milliseconds.push(elapsed);
    }
    milliseconds.sort_by(f64::total_cmp);
    println!(
        "PostgreSQL vector search: min={:.3} median={:.3} max={:.3} ms",
        milliseconds[0], milliseconds[2], milliseconds[4]
    );
    txn.rollback().await?;
    db.close().await?;
    Ok(())
}
