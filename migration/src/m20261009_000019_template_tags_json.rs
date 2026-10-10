//! Store a template's tags as a JSON array on both backends.
//!
//! PostgreSQL held them in a `TEXT[]` column and SQLite in JSON-encoded `TEXT`, so the generated entity's `Vec<String>` bound an array that SQLite cannot take and every template write panicked there.
//! A JSON array is one shape for both: `JSONB` on PostgreSQL (the existing tags are converted in place and keep their order) and the `TEXT` column SQLite already has.

use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for sql in [
            "DROP INDEX IF EXISTS templates_tags_idx",
            "ALTER TABLE template_templates ALTER COLUMN tags DROP DEFAULT",
            "ALTER TABLE template_templates ALTER COLUMN tags TYPE JSONB USING to_jsonb(tags)",
            "ALTER TABLE template_templates ALTER COLUMN tags SET DEFAULT '[]'::jsonb",
            "CREATE INDEX templates_tags_idx ON template_templates USING gin(tags)",
        ] {
            helpers::pg_only(manager, sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // `ALTER ... TYPE ... USING` cannot hold a subquery, so the array is rebuilt in a new column.
        for sql in [
            "DROP INDEX IF EXISTS templates_tags_idx",
            "ALTER TABLE template_templates ADD COLUMN tags_array TEXT[] NOT NULL DEFAULT '{}'",
            "UPDATE template_templates SET tags_array = ARRAY( \
                 SELECT e FROM jsonb_array_elements_text(tags) WITH ORDINALITY AS a(e, n) ORDER BY n)",
            "ALTER TABLE template_templates DROP COLUMN tags",
            "ALTER TABLE template_templates RENAME COLUMN tags_array TO tags",
            "CREATE INDEX templates_tags_idx ON template_templates USING gin(tags)",
        ] {
            helpers::pg_only(manager, sql).await?;
        }
        Ok(())
    }
}
