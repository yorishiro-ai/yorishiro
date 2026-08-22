use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // password_hash is nullable from the start: the old DDL created it NOT NULL then
        // dropped that constraint once OAuth landed (a two-step historical replay), but a fresh
        // table can just start in the end state. oauth_provider/oauth_subject_id are folded
        // into the same create_table call for the same reason, rather than a separate ALTER.
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
        // never a mix, and never neither (a user's login method must be determinable at a
        // glance). sea_query's table-level .check() was not used here so the constraint name
        // matches the old DDL exactly for traceability; a plain execute_unprepared right after
        // create_table is equally simple.
        db.execute_unprepared(
            "ALTER TABLE identity_users \
             ADD CONSTRAINT users_auth_method_check CHECK ( \
               (password_hash IS NOT NULL AND oauth_provider IS NULL AND oauth_subject_id IS NULL) \
               OR (oauth_provider IS NOT NULL AND oauth_subject_id IS NOT NULL) \
             );",
        )
        .await?;

        // The subject id ("sub" claim) an identity provider issues is only unique within that
        // provider, so the lookup/uniqueness key is the pair, not either column alone,
        // otherwise two different providers that happen to both hand out subject id "1" would
        // collide. Partial (WHERE oauth_provider IS NOT NULL) so password-only rows, which
        // leave both columns NULL, never collide with each other on the unique index.
        db.execute_unprepared(
            "CREATE UNIQUE INDEX users_oauth_identity_idx \
             ON identity_users (oauth_provider, oauth_subject_id) \
             WHERE oauth_provider IS NOT NULL;",
        )
        .await?;

        // No RLS and no GRANT for this table anywhere in the old DDL (grepped both the RLS
        // section and the grants section): identity_users is a control-plane table reached only
        // through the migration-role identity pool (see src/db.rs), never a tenant-scoped
        // request connection, so yorishiro_app has no business touching it directly.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("identity_users")).to_owned())
            .await
    }
}
