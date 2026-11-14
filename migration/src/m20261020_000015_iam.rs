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
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("iam_tenant_roots"))
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).primary_key())
                    .col(
                        helpers::uuid_col(manager, Alias::new("user_id"))
                            .not_null()
                            .unique_key(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("iam_tenant_roots"))
                            .from_col(Alias::new("tenant_id"))
                            .from_col(Alias::new("user_id"))
                            .to_tbl(Alias::new("tenant_memberships"))
                            .to_col(Alias::new("tenant_id"))
                            .to_col(Alias::new("user_id")),
                    )
                    .to_owned(),
            )
            .await?;
        for table in ["iam_groups", "iam_policies"] {
            let mut create = Table::create();
            create
                .table(Alias::new(table))
                .col(helpers::uuidv7_pk(manager))
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                .col(ColumnDef::new(Alias::new("name")).text().not_null());
            if table == "iam_policies" {
                create.col(helpers::json_col(manager, Alias::new("document")).not_null());
            }
            for timestamp in helpers::timestamps(manager) {
                create.col(timestamp);
            }
            create.foreign_key(
                ForeignKey::create()
                    .from(Alias::new(table), Alias::new("tenant_id"))
                    .to(Alias::new("tenant_tenants"), Alias::new("id"))
                    .on_delete(ForeignKeyAction::Cascade),
            );
            manager.create_table(create.to_owned()).await?;
            for (suffix, column) in [("id", "id"), ("name", "name")] {
                manager
                    .create_index(
                        Index::create()
                            .name(format!("uq_{table}_tenant_id_{suffix}"))
                            .table(Alias::new(table))
                            .col(Alias::new("tenant_id"))
                            .col(Alias::new(column))
                            .unique()
                            .to_owned(),
                    )
                    .await?;
            }
        }

        for (table, left, left_table, right, right_table) in [
            (
                "iam_group_memberships",
                "group_id",
                "iam_groups",
                "user_id",
                "tenant_memberships",
            ),
            (
                "iam_user_policy_attachments",
                "user_id",
                "tenant_memberships",
                "policy_id",
                "iam_policies",
            ),
            (
                "iam_group_policy_attachments",
                "group_id",
                "iam_groups",
                "policy_id",
                "iam_policies",
            ),
        ] {
            let mut create = Table::create();
            create
                .table(Alias::new(table))
                .col(helpers::uuidv7_pk(manager))
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                .col(helpers::uuid_col(manager, Alias::new(left)).not_null())
                .col(helpers::uuid_col(manager, Alias::new(right)).not_null())
                .col(helpers::created_at(manager));
            for (column, target) in [(left, left_table), (right, right_table)] {
                create.foreign_key(
                    ForeignKey::create()
                        .from_tbl(Alias::new(table))
                        .from_col(Alias::new("tenant_id"))
                        .from_col(Alias::new(column))
                        .to_tbl(Alias::new(target))
                        .to_col(Alias::new("tenant_id"))
                        .to_col(Alias::new(if target == "tenant_memberships" {
                            "user_id"
                        } else {
                            "id"
                        }))
                        .on_delete(ForeignKeyAction::Cascade),
                );
            }
            manager.create_table(create.to_owned()).await?;
            manager
                .create_index(
                    Index::create()
                        .name(format!("uq_{table}_subjects"))
                        .table(Alias::new(table))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new(left))
                        .col(Alias::new(right))
                        .unique()
                        .to_owned(),
                )
                .await?;
            manager
                .create_index(
                    Index::create()
                        .name(format!("idx_{table}_reverse"))
                        .table(Alias::new(table))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new(right))
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("iam_audit_events"))
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("user_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("api_key_id")).not_null())
                    .col(ColumnDef::new(Alias::new("action")).text().not_null())
                    .col(helpers::json_col(manager, Alias::new("detail")).not_null())
                    .col(helpers::created_at(manager))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Alias::new("iam_audit_events"), Alias::new("tenant_id"))
                            .to(Alias::new("tenant_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_iam_audit_events_tenant_created")
                    .table(Alias::new("iam_audit_events"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("created_at"))
                    .to_owned(),
            )
            .await?;
        for table in [
            "iam_tenant_roots",
            "iam_audit_events",
            "iam_groups",
            "iam_policies",
            "iam_group_memberships",
            "iam_user_policy_attachments",
            "iam_group_policy_attachments",
        ] {
            helpers::enable_rls_with_policy(
                manager,
                table,
                "tenant_isolation",
                "tenant_id",
                "app.current_tenant",
                false,
            )
            .await?;
            let privileges = if matches!(table, "iam_tenant_roots" | "iam_audit_events") {
                "SELECT, INSERT"
            } else {
                "SELECT, INSERT, UPDATE, DELETE"
            };
            helpers::grant(manager, privileges, table).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "iam_audit_events",
            "iam_group_policy_attachments",
            "iam_user_policy_attachments",
            "iam_group_memberships",
            "iam_policies",
            "iam_groups",
            "iam_tenant_roots",
        ] {
            manager
                .drop_table(Table::drop().table(Alias::new(table)).to_owned())
                .await?;
        }
        Ok(())
    }
}
