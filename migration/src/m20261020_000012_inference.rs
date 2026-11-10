use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        up_inference_jobs(manager).await?;
        up_inference_proposals(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        down_inference_proposals(manager).await?;
        down_inference_jobs(manager).await?;
        Ok(())
    }
}

async fn up_inference_jobs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let [created_at, updated_at] = helpers::timestamps(manager);
    let table = Table::create()
        .table(Alias::new("inference_jobs"))
        .if_not_exists()
        .col(helpers::uuidv7_pk(manager))
        .col(
            ColumnDef::new(Alias::new("attempt"))
                .integer()
                .not_null()
                .default(0),
        )
        .col(
            ColumnDef::new(Alias::new("proposed"))
                .big_integer()
                .not_null()
                .default(0),
        )
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(ColumnDef::new(Alias::new("schema_name")).text().not_null())
        .col(ColumnDef::new(Alias::new("status")).text().not_null())
        .col(
            ColumnDef::new(Alias::new("applied"))
                .big_integer()
                .not_null()
                .default(0),
        )
        .col(
            ColumnDef::new(Alias::new("skipped"))
                .big_integer()
                .not_null()
                .default(0),
        )
        .col(ColumnDef::new(Alias::new("error")).text())
        .col(created_at)
        .col(updated_at)
        .foreign_key(
            ForeignKey::create()
                .name("fk_inference_jobs_workspace_id")
                .from(Alias::new("inference_jobs"), Alias::new("workspace_id"))
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned();

    helpers::create_table_with_checks(
        manager,
        "inference_jobs",
        table,
        &[(
            "inference_jobs_status_check",
            "status IN ('queued', 'running', 'completed', 'failed')",
        )],
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("inference_jobs_workspace_id_created_at_idx")
                .table(Alias::new("inference_jobs"))
                .col(Alias::new("workspace_id"))
                .col((
                    Alias::new("created_at"),
                    sea_orm_migration::sea_orm::sea_query::IndexOrder::Desc,
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("inference_jobs_one_running_per_workspace_idx")
                .table(Alias::new("inference_jobs"))
                .col(Alias::new("workspace_id"))
                .unique()
                .and_where(Expr::col(Alias::new("status")).eq("running"))
                .to_owned(),
        )
        .await?;
    helpers::enable_rls_with_policy(
        manager,
        "inference_jobs",
        "workspace_isolation",
        "workspace_id",
        "app.current_workspace",
        false,
    )
    .await?;
    helpers::grant(manager, "SELECT, INSERT, UPDATE", "inference_jobs").await?;
    Ok(())
}

async fn down_inference_jobs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(Table::drop().table(Alias::new("inference_jobs")).to_owned())
        .await?;

    Ok(())
}

async fn up_inference_proposals(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let [created_at, updated_at] = helpers::timestamps(manager);
    let table = Table::create()
        .table(Alias::new("inference_proposals"))
        .if_not_exists()
        .col(helpers::uuidv7_pk(manager))
        .col(helpers::uuid_col(manager, Alias::new("job_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("workspace_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("entity_id")).not_null())
        .col(helpers::uuid_col(manager, Alias::new("schema_id")).not_null())
        .col(
            ColumnDef::new(Alias::new("schema_version"))
                .integer()
                .not_null(),
        )
        .col(ColumnDef::new(Alias::new("source_field")).text().not_null())
        .col(helpers::json_col(manager, Alias::new("proposed")).not_null())
        .col(ColumnDef::new(Alias::new("status")).text().not_null())
        .col(created_at)
        .col(updated_at)
        .foreign_key(
            ForeignKey::create()
                .name("fk_inference_proposals_workspace_id")
                .from(
                    Alias::new("inference_proposals"),
                    Alias::new("workspace_id"),
                )
                .to(Alias::new("workspace_workspaces"), Alias::new("id"))
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned();

    helpers::create_table_with_checks(
        manager,
        "inference_proposals",
        table,
        &[(
            "inference_proposals_status_check",
            "status IN ('pending', 'confirmed', 'rejected', 'discarded', 'stale', 'invalid')",
        )],
    )
    .await?;

    manager
        .create_index(
            Index::create()
                .name("inference_proposals_delivery_key")
                .table(Alias::new("inference_proposals"))
                .unique()
                .col(Alias::new("workspace_id"))
                .col(Alias::new("job_id"))
                .col(Alias::new("entity_id"))
                .col(Alias::new("schema_id"))
                .col(Alias::new("schema_version"))
                .col(Alias::new("source_field"))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("inference_proposals_workspace_job_idx")
                .table(Alias::new("inference_proposals"))
                .col(Alias::new("workspace_id"))
                .col(Alias::new("job_id"))
                .col(Alias::new("status"))
                .to_owned(),
        )
        .await?;

    helpers::enable_rls_with_policy(
        manager,
        "inference_proposals",
        "workspace_isolation",
        "workspace_id",
        "app.current_workspace",
        true,
    )
    .await?;
    helpers::grant(
        manager,
        "SELECT, INSERT, UPDATE, DELETE",
        "inference_proposals",
    )
    .await?;

    Ok(())
}

async fn down_inference_proposals(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(
            Table::drop()
                .table(Alias::new("inference_proposals"))
                .to_owned(),
        )
        .await?;

    Ok(())
}
