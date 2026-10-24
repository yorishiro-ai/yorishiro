use sea_orm::{
    ConnectionTrait, DbErr, ExprTrait,
    sea_query::{Alias, Expr, Query},
};
use uuid::Uuid;

/// Inserts a snapshot from the current entity row in one statement.
/// SeaQuery keeps the read and insert atomic on both supported backends.
pub(super) async fn insert_snapshot(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    job_id: Uuid,
) -> Result<u64, DbErr> {
    use crate::models::_entities::{entity_entities, entity_snapshots};
    let source = Query::select()
        .expr(Expr::val(Uuid::now_v7()))
        .expr(Expr::val(job_id))
        .columns([
            entity_entities::Column::WorkspaceId,
            entity_entities::Column::Id,
            entity_entities::Column::SchemaId,
            entity_entities::Column::SchemaVersion,
            entity_entities::Column::Data,
        ])
        .from(Alias::new("entity_entities"))
        .and_where(Expr::col(entity_entities::Column::WorkspaceId).eq(workspace_id))
        .and_where(Expr::col(entity_entities::Column::Id).eq(entity_id))
        .to_owned();
    let insert = Query::insert()
        .into_table(Alias::new("entity_snapshots"))
        .columns([
            entity_snapshots::Column::Id,
            entity_snapshots::Column::JobId,
            entity_snapshots::Column::WorkspaceId,
            entity_snapshots::Column::EntityId,
            entity_snapshots::Column::SchemaId,
            entity_snapshots::Column::SchemaVersion,
            entity_snapshots::Column::Data,
        ])
        .select_from(source)
        .map_err(|error| DbErr::Custom(error.to_string()))?
        .to_owned();
    conn.execute_raw(conn.get_database_backend().build(&insert))
        .await
        .map(|result| result.rows_affected())
}
