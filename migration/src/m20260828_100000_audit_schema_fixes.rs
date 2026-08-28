use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The trigger body both backends share, differing only in how each spells "now".
/// One string with the timestamp expression substituted, so the two versions cannot drift in the part that matters: the `WHERE` clause deciding which rows detach.
fn detach_body(now: &str) -> String {
    format!(
        "UPDATE content_schemas \
            SET origin_status = 'detached', updated_at = {now} \
          WHERE origin_template_id = OLD.id \
            AND origin_status = 'linked';"
    )
}

/// The trigger body as it stood before this migration, for `down()` to restore.
const DETACH_BODY_UNSTAMPED: &str = "UPDATE content_schemas \
        SET origin_status = 'detached' \
      WHERE origin_template_id = OLD.id \
        AND origin_status = 'linked';";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. `identity_workspace_worker_classes.worker_class` accepted any string.
        //
        // Its siblings carry this constraint (`identity_tenant_memberships.role`, `identity_api_keys.scope`, the audit log's `action`), for the reason `action`'s own migration states: a CHECK is what stops a typo'd value from silently becoming a fourth thing nothing filters on.
        // The original migration argued the serde round-trip made it unnecessary, which guarantees only that *this* writer emits a valid value: a manual UPDATE, a data-fix script, or a future code path reaching the column directly are all outside that guarantee, and this column decides which worker process dequeues a job.
        // The three values are `WorkerClass::as_db_str`'s own output, so a variant added there without adding it here fails closed at the database rather than routing jobs to a queue nothing reads.
        //
        // **PostgreSQL only, and SQLite is left without this constraint.** SQLite's `ALTER TABLE` cannot add one to an existing table, and the only way to gain it there is to rebuild the table and copy every row.
        // That is a large, risky migration to close a gap on the tier documented as "trying Yorishiro out or personal use" (`docs/sqlite.md`), so it is not done here: on SQLite this column still accepts any string, and a deployment that needs the constraint needs PostgreSQL.
        helpers::pg_only(
            manager,
            "ALTER TABLE identity_workspace_worker_classes \
             ADD CONSTRAINT identity_workspace_worker_classes_worker_class_check \
             CHECK (worker_class IN ('tenant_private', 'official', 'shared'))",
        )
        .await?;

        // 2. `identity_templates.created_by` had no ON DELETE, so it defaulted to NO ACTION and a user who had authored a template could not be deleted at all.
        //
        // SET NULL, matching `fork_of` on this same table, and chosen rather than inherited: the column is nullable, a template outlives the account that wrote it, and the alternatives are both wrong here.
        // CASCADE would delete a tenant's templates because an author closed their account, destroying data belonging to the tenant rather than to the user.
        // RESTRICT keeps today's behaviour, where the only way to delete such a user is to delete or re-author every template they ever wrote.
        // Losing the authorship attribution is the acceptable half of that trade; losing the template is not.
        //
        // **PostgreSQL only, and this is a real gap rather than an engine detail.** `identity_templates` exists on SQLite too: `m20260822_100600_templates.rs` creates it unconditionally, and that file's `pg_only`/`sqlite_only` calls cover the `tags` column's type and a GIN index, not the table itself.
        // SQLite cannot alter a foreign key's action in place, so `created_by` keeps NO ACTION there and deleting a user who authored a template still fails with `FOREIGN KEY constraint failed` (measured directly against a SQLite file, not inferred).
        // Closing it means the same table rebuild the CHECK above declines, for the same reason.
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
        // The table is not append-only: the `detach_orphaned_schema_origin` trigger created alongside it rewrites `origin_status` in place when an upstream template is deleted, so a row could change with nothing recording when.
        //
        // Added through the schema builder rather than raw SQL because this table exists on SQLite too, and that backend's `ALTER TABLE` cannot do `ALTER COLUMN ... SET NOT NULL`.
        // Nullable for the same reason, and because SQLite refuses a non-constant default on `ADD COLUMN` outright (measured: `Cannot add a column with non-constant default`), so an existing table cannot be given a `now()` default there.
        //
        // `None` therefore means exactly one thing: the row has not been written by any path since this column was added.
        // Every write path stamps it from here on (both triggers below, `content_schemas::create_schema`, and that module's archival `update_many`), so `None` is purely historical and its population only shrinks; it is not a state a new or modified row can enter.
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

        // PostgreSQL can carry a default; SQLite cannot, per the measurement above.
        // Which is why the stamping is not left to the column on either backend: the guarantee has to hold where the weaker engine is, so every write path stamps explicitly and this default is belt-and-braces for Postgres rather than the mechanism.
        helpers::pg_only(
            manager,
            "ALTER TABLE content_schemas ALTER COLUMN updated_at SET DEFAULT now()",
        )
        .await?;

        // The triggers are replaced rather than left alone, because they are the specific in-place rewrite this column exists to record.
        // A trigger that detaches a schema without stamping `updated_at` would leave the column recording every change except the one its own justification names.
        helpers::pg_only(
            manager,
            &format!(
                "CREATE OR REPLACE FUNCTION detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
                 BEGIN
                   {}
                   RETURN OLD;
                 END;
                 $$ LANGUAGE plpgsql SECURITY DEFINER;",
                detach_body("now()")
            ),
        )
        .await?;

        // SQLite has no CREATE OR REPLACE for triggers, so the existing one is dropped first.
        // The name and `AFTER DELETE` timing are carried over unchanged from `m20260822_100800_content_schemas.rs`, so this replaces that trigger rather than adding a second one beside it.
        helpers::sqlite_only(
            manager,
            "DROP TRIGGER IF EXISTS templates_detach_schema_origins",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            &format!(
                "CREATE TRIGGER templates_detach_schema_origins
                 AFTER DELETE ON identity_templates
                 FOR EACH ROW
                 BEGIN
                   {}
                 END;",
                detach_body("CURRENT_TIMESTAMP")
            ),
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Both triggers go back to their unstamped form *before* the column is dropped, so the reversal never leaves a trigger writing a column that no longer exists.
        helpers::pg_only(
            manager,
            &format!(
                "CREATE OR REPLACE FUNCTION detach_orphaned_schema_origin() RETURNS TRIGGER AS $$
                 BEGIN
                   {DETACH_BODY_UNSTAMPED}
                   RETURN OLD;
                 END;
                 $$ LANGUAGE plpgsql SECURITY DEFINER;"
            ),
        )
        .await?;

        helpers::sqlite_only(
            manager,
            "DROP TRIGGER IF EXISTS templates_detach_schema_origins",
        )
        .await?;
        helpers::sqlite_only(
            manager,
            &format!(
                "CREATE TRIGGER templates_detach_schema_origins
                 AFTER DELETE ON identity_templates
                 FOR EACH ROW
                 BEGIN
                   {DETACH_BODY_UNSTAMPED}
                 END;"
            ),
        )
        .await?;

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
