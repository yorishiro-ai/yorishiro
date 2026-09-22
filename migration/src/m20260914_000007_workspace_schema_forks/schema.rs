use sea_orm_migration::prelude::*;

use crate::helpers;

mod forks;
mod heads;

pub(super) async fn up(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    forks::create(manager).await?;
    heads::create(manager).await?;
    Ok(())
}

pub(super) async fn down(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    helpers::pg_only(
                manager,
                "DROP TRIGGER IF EXISTS workspace_schema_fork_heads_integrity ON workspace_schema_fork_heads;
                 DROP TRIGGER IF EXISTS workspace_schema_forks_integrity ON workspace_schema_forks;
                 DROP FUNCTION IF EXISTS workspace_schema_fork_edges(uuid, uuid);
                 DROP FUNCTION IF EXISTS workspace_schema_fork_latest_source(uuid, uuid, text);
                 DROP FUNCTION IF EXISTS workspace_schema_fork_source_by_id(uuid, uuid, uuid);
                 DROP FUNCTION IF EXISTS check_workspace_schema_fork_head_integrity();
                 DROP FUNCTION IF EXISTS check_workspace_schema_fork_integrity();",
            )
            .await?;
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("workspace_schema_fork_heads"))
                .to_owned(),
        )
        .await?;
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("workspace_schema_forks"))
                .to_owned(),
        )
        .await?;
    Ok(())
}
