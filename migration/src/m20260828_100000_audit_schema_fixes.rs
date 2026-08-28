use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. `identity_workspace_worker_classes.worker_class` accepted any string.
        //
        // Its siblings carry this constraint (`identity_tenant_memberships.role`, `identity_api_keys.scope`, the audit log's `action`), and the reason is the same one `action`'s own migration states: a CHECK is what stops a typo'd value from silently becoming a fourth thing nothing filters on.
        // The original migration argued the serde round-trip made it unnecessary, which guarantees only that *this* writer emits a valid value: a manual UPDATE, a data-fix script, or a future code path reaching the column directly are all outside that guarantee, and this column decides which worker process dequeues a job.
        // The three values are `WorkerClass::as_db_str`'s own output, so a variant added there without adding it here fails closed at the database rather than routing jobs to a queue nothing reads.
        //
        // Postgres only, and this one is a real gap rather than an engine-specific optimization: SQLite's ALTER TABLE cannot add a constraint to an existing table, and rebuilding the table to gain one is a larger change than this migration should make.
        // `helpers::create_table_with_checks` is how a table gets an inline CHECK on both backends, and it only applies at creation time.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspace_worker_classes \
             ADD CONSTRAINT identity_workspace_worker_classes_worker_class_check \
             CHECK (worker_class IN ('tenant_private', 'official', 'shared'))",
        )
        .await?;

        // 2. `identity_templates.created_by` had no ON DELETE, so it defaulted to NO ACTION and a user who had authored a template could not be deleted at all.
        //
        // SET NULL, matching `fork_of` on this same table, and chosen rather than inherited: the column is nullable, a template outlives the account that wrote it, and the alternative actions are both wrong here.
        // CASCADE would delete a tenant's templates because an author's account was closed, which destroys data belonging to the tenant rather than to the user.
        // RESTRICT keeps today's behaviour, where the only way to delete such a user is to delete or re-author every template they ever wrote.
        // Losing the authorship attribution is the acceptable half of that trade; losing the template is not.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             DROP CONSTRAINT fk_identity_templates_created_by",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             ADD CONSTRAINT fk_identity_templates_created_by \
             FOREIGN KEY (created_by) REFERENCES identity_users(id) ON DELETE SET NULL",
        )
        .await?;

        // 3. `content_schemas` had no `updated_at`, while every other mutable table here does.
        //
        // The table is not append-only: the `origin_status` trigger created alongside it rewrites rows in place when an upstream template changes, so a row could change with nothing recording when.
        // Added through the schema builder rather than raw SQL because this table exists on SQLite too, and that backend's ALTER TABLE cannot do `ALTER COLUMN ... SET NOT NULL`.
        // Nullable rather than NOT NULL for the same reason: a single ADD COLUMN is all SQLite supports here, and a NOT NULL column added to a table with existing rows needs a default SQLite will not accept a non-constant one for.
        // `None` therefore means "last modified at some point before this column existed", which is honest, where backfilling `now()` would assert an edit that never happened and backfilling `created_at` would assert the row has never changed.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("content_schemas"))
                    .add_column(
                        ColumnDef::new(Alias::new("updated_at"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("content_schemas"))
                    .drop_column(Alias::new("updated_at"))
                    .to_owned(),
            )
            .await?;

        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             DROP CONSTRAINT fk_identity_templates_created_by",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_templates \
             ADD CONSTRAINT fk_identity_templates_created_by \
             FOREIGN KEY (created_by) REFERENCES identity_users(id)",
        )
        .await?;

        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspace_worker_classes \
             DROP CONSTRAINT identity_workspace_worker_classes_worker_class_check",
        )
        .await?;

        Ok(())
    }
}
