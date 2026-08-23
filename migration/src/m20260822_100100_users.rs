use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // password_hash is nullable: a user may be OAuth-provisioned instead of password-based.
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_users"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(
                        ColumnDef::new(Alias::new("email"))
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Alias::new("password_hash")).text())
                    .col(ColumnDef::new(Alias::new("display_name")).text())
                    .col(ColumnDef::new(Alias::new("oauth_provider")).text())
                    .col(ColumnDef::new(Alias::new("oauth_subject_id")).text())
                    .col(helpers::created_at())
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // Every row is either password-authenticated (password_hash set, oauth_* both NULL) or
        // OAuth-provisioned (oauth_provider + oauth_subject_id set, password_hash may be NULL),
        // never a mix and never neither.
        db.execute_unprepared(
            "ALTER TABLE identity_users \
             ADD CONSTRAINT users_auth_method_check CHECK ( \
               (password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL) \
               OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL) \
             );",
        )
        .await?;

        // The subject id ("sub" claim) is only unique within its provider, so the uniqueness key is the pair, not either column alone.
        // Partial (WHERE oauth_provider IS NOT NULL) so password-only rows, which leave both columns NULL, never collide on the unique index.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX users_oauth_identity_idx \
             ON identity_users (oauth_provider, oauth_subject_id) \
             WHERE oauth_provider IS NOT NULL;",
        )
        .await?;

        // No RLS and no GRANT for this table: identity_users is a control-plane table reached
        // only through the migration-role identity pool (see src/db.rs), never a tenant-scoped
        // request connection, so yorishiro_app has no business touching it directly.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("identity_users")).to_owned())
            .await
    }
}
