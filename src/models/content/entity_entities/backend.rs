use sea_orm::{ConnectionTrait, DbErr, Statement};
use uuid::Uuid;

/// Inserts a snapshot from the current entity row in one statement.
/// SeaORM cannot express this `INSERT ... SELECT` while preserving the snapshot race semantics.
pub(super) async fn insert_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<u64, DbErr> {
    conn.execute_raw(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO entity_snapshots \
            (job_id, workspace_id, entity_id, schema_id, schema_version, data) \
         SELECT $1, workspace_id, id, schema_id, schema_version, data \
           FROM entity_entities \
          WHERE workspace_id = $2 AND id = $3",
        [job_id.into(), workspace_id.into(), entity_id.into()],
    ))
    .await
    .map(|result| result.rows_affected())
}
