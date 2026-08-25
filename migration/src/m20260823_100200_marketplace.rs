use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_template_versions"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("template_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("version")).integer().not_null())
                    .col(
                        ColumnDef::new(Alias::new("definition"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("changelog")).text())
                    // draft: visible only to the owning tenant.
                    // pre: published but announced as unstable.
                    // stable: the version an installer gets by default.
                    .col(
                        ColumnDef::new(Alias::new("status"))
                            .text()
                            .not_null()
                            .default("draft"),
                    )
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(helpers::created_at())
                    .check(Expr::cust("status IN ('draft', 'pre', 'stable')"))
                    .index(
                        Index::create()
                            .name("template_versions_template_id_version_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("version")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_versions_template_id")
                            .from(
                                Alias::new("identity_template_versions"),
                                Alias::new("template_id"),
                            )
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_versions_created_by")
                            .from(
                                Alias::new("identity_template_versions"),
                                Alias::new("created_by"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_versions_template_idx")
                    .table(Alias::new("identity_template_versions"))
                    .col(Alias::new("template_id"))
                    .col((Alias::new("version"), IndexOrder::Desc))
                    .to_owned(),
            )
            .await?;

        // One review per tenant per template: a tenant that used a template twice does not get two votes, and updating an opinion is an UPDATE (upsert_review's ON CONFLICT) rather than a second row.
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_template_reviews"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk(manager))
                    .col(ColumnDef::new(Alias::new("template_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("rating"))
                            .small_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("comment")).text())
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(helpers::created_at())
                    .col(
                        ColumnDef::new(Alias::new("updated_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .check(Expr::cust("rating BETWEEN 1 AND 5"))
                    .index(
                        Index::create()
                            .name("template_reviews_template_id_tenant_id_key")
                            .unique()
                            .col(Alias::new("template_id"))
                            .col(Alias::new("tenant_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_template_id")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("template_id"),
                            )
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_tenant_id")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_template_reviews_created_by")
                            .from(
                                Alias::new("identity_template_reviews"),
                                Alias::new("created_by"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("template_reviews_template_idx")
                    .table(Alias::new("identity_template_reviews"))
                    .col(Alias::new("template_id"))
                    .to_owned(),
            )
            .await?;

        // No RLS, no GRANT to yorishiro_app on either table, matching identity_templates itself: both are read in exactly the same paths (the repository layer, as the owner role, scoping by tenant in the query) and are never reached through the tenant pool.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_template_reviews"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_template_versions"))
                    .to_owned(),
            )
            .await
    }
}
