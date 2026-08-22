use sea_orm_migration::prelude::*;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let [created_at, updated_at] = helpers::timestamps();
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("identity_templates"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("description")).text())
                    .col(ColumnDef::new(Alias::new("definition")).json_binary().not_null())
                    .col(ColumnDef::new(Alias::new("locale")).text())
                    .col(
                        ColumnDef::new(Alias::new("visibility"))
                            .text()
                            .not_null()
                            .default("tenant"),
                    )
                    .col(ColumnDef::new(Alias::new("author")).text())
                    .col(ColumnDef::new(Alias::new("fork_of")).uuid())
                    .col(ColumnDef::new(Alias::new("created_by")).uuid())
                    .col(created_at)
                    .col(updated_at)
                    .check(Expr::cust("visibility IN ('tenant', 'community')"))
                    .index(
                        Index::create()
                            .name("templates_tenant_id_name_key")
                            .unique()
                            .col(Alias::new("tenant_id"))
                            .col(Alias::new("name")),
                    )
                    // No ON DELETE action on this particular FK (line 136 of the old DDL),
                    // unlike every other tenant_id FK in this schema: RESTRICT/NO ACTION is
                    // the default, so deleting a tenant with templates fails loudly instead
                    // of cascading them away.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_tenant_id")
                            .from(Alias::new("identity_templates"), Alias::new("tenant_id"))
                            .to(Alias::new("identity_tenants"), Alias::new("id")),
                    )
                    // Self-referential: deleting a template others were forked from must
                    // leave the forks usable, losing only the pointer back.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_fork_of")
                            .from(Alias::new("identity_templates"), Alias::new("fork_of"))
                            .to(Alias::new("identity_templates"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_templates_created_by")
                            .from(Alias::new("identity_templates"), Alias::new("created_by"))
                            .to(Alias::new("identity_users"), Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        // sea_query's ColumnDef has no TEXT[] helper, so this column is added as raw SQL
        // right after create_table.
        db.execute_unprepared(
            "ALTER TABLE identity_templates ADD COLUMN tags TEXT[] NOT NULL DEFAULT '{}';",
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("templates_tenant_id_idx")
                    .table(Alias::new("identity_templates"))
                    .col(Alias::new("tenant_id"))
                    .to_owned(),
            )
            .await?;

        // GIN index over a TEXT[] column: create_index has no operator-class expression for
        // this, so it goes through raw SQL.
        db.execute_unprepared(
            "CREATE INDEX templates_tags_idx ON identity_templates USING gin(tags);",
        )
        .await?;

        // No RLS on this table, deliberately: template queries run as the owner through the
        // repository layer, which scopes by tenant in the query itself, because a policy
        // would have to read the table the app role holds no grant on, and a policy the role
        // cannot evaluate fails the query rather than filtering it.
        //
        // No GRANT either, for the same reason: yorishiro_app is never the role that reaches
        // this table, so there is nothing to grant it.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("identity_templates")).to_owned())
            .await
    }
}
