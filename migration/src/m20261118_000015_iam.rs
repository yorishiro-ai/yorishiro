use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const APPEND_ONLY: &str = "SELECT, INSERT";
const MUTABLE: &str = "SELECT, INSERT, UPDATE, DELETE";

#[expect(
    clippy::too_many_lines,
    reason = "ordered migration DDL keeps table definitions, constraints, and grants together"
)]
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
        for (table, names, name_key) in [
            ("iam_groups", &["name", "name_key"][..], "name_key"),
            ("iam_policies", &["name"][..], "name"),
        ] {
            let mut create = Table::create();
            create
                .table(Alias::new(table))
                .col(helpers::uuidv7_pk(manager))
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null());
            for name in names {
                create.col(ColumnDef::new(Alias::new(*name)).text().not_null());
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
            manager.create_table(create).await?;
            for column in ["id", name_key] {
                manager
                    .create_index(
                        Index::create()
                            .name(format!("uq_{table}_tenant_id_{column}"))
                            .table(Alias::new(table))
                            .col(Alias::new("tenant_id"))
                            .col(Alias::new(column))
                            .unique()
                            .to_owned(),
                    )
                    .await?;
            }
        }

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("iam_policy_versions"))
                    .col(helpers::uuidv7_pk(manager))
                    .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                    .col(helpers::uuid_col(manager, Alias::new("policy_id")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("version_number"))
                            .integer()
                            .not_null(),
                    )
                    .col(helpers::json_col(manager, Alias::new("document")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("is_default"))
                            .boolean()
                            .not_null(),
                    )
                    .col(helpers::created_at(manager))
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("iam_policy_versions"))
                            .from_col(Alias::new("tenant_id"))
                            .from_col(Alias::new("policy_id"))
                            .to_tbl(Alias::new("iam_policies"))
                            .to_col(Alias::new("tenant_id"))
                            .to_col(Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_iam_policy_versions_policy_number")
                    .table(Alias::new("iam_policy_versions"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("policy_id"))
                    .col(Alias::new("version_number"))
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("uq_iam_policy_versions_default")
                    .table(Alias::new("iam_policy_versions"))
                    .col(Alias::new("tenant_id"))
                    .col(Alias::new("policy_id"))
                    .unique()
                    .and_where(Expr::col(Alias::new("is_default")).eq(true))
                    .to_owned(),
            )
            .await?;

        for (table, left, right) in [
            (
                "iam_group_memberships",
                ("group_id", "iam_groups", "id"),
                ("user_id", "tenant_memberships", "user_id"),
            ),
            (
                "iam_user_policy_attachments",
                ("user_id", "tenant_memberships", "user_id"),
                ("policy_id", "iam_policies", "id"),
            ),
            (
                "iam_group_policy_attachments",
                ("group_id", "iam_groups", "id"),
                ("policy_id", "iam_policies", "id"),
            ),
        ] {
            let mut create = Table::create();
            create
                .table(Alias::new(table))
                .col(helpers::uuidv7_pk(manager))
                .col(helpers::uuid_col(manager, Alias::new("tenant_id")).not_null())
                .col(helpers::uuid_col(manager, Alias::new(left.0)).not_null())
                .col(helpers::uuid_col(manager, Alias::new(right.0)).not_null())
                .col(helpers::created_at(manager));
            for (column, target, target_column) in [left, right] {
                if target == "iam_policies" {
                    // Managed policies have no row, so the policy id cannot reference one.
                    continue;
                }
                create.foreign_key(
                    ForeignKey::create()
                        .from_tbl(Alias::new(table))
                        .from_col(Alias::new("tenant_id"))
                        .from_col(Alias::new(column))
                        .to_tbl(Alias::new(target))
                        .to_col(Alias::new("tenant_id"))
                        .to_col(Alias::new(target_column))
                        .on_delete(ForeignKeyAction::Cascade),
                );
            }
            manager.create_table(create).await?;
            manager
                .create_index(
                    Index::create()
                        .name(format!("uq_{table}_subjects"))
                        .table(Alias::new(table))
                        .col(Alias::new("tenant_id"))
                        .col(Alias::new(left.0))
                        .col(Alias::new(right.0))
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
                        .col(Alias::new(right.0))
                        .to_owned(),
                )
                .await?;
        }
        for (table, privileges) in [
            ("iam_tenant_roots", APPEND_ONLY),
            ("iam_groups", MUTABLE),
            ("iam_policies", MUTABLE),
            ("iam_policy_versions", MUTABLE),
            ("iam_group_memberships", MUTABLE),
            ("iam_user_policy_attachments", MUTABLE),
            ("iam_group_policy_attachments", MUTABLE),
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
            helpers::grant(manager, privileges, table).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            "iam_group_policy_attachments",
            "iam_user_policy_attachments",
            "iam_group_memberships",
            "iam_policy_versions",
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
