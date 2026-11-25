//! Replaces IAM roles with groups.
//!
//! Each role becomes a new group of the same name, keeping its members and attached policies.
//! A group that already has the role's name is a different principal with different members, so it is renamed first.
//! Merging would hand the role's policies to that group's members.
//! Stored policy documents lose the removed role actions, so every tenant keeps documents the application can parse.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, QueryResult};
use uuid::Uuid;

use crate::helpers;

#[derive(DeriveMigrationName)]
pub(crate) struct Migration;

const REMOVED_ACTIONS: [&str; 11] = [
    "role_list",
    "role_get",
    "role_create",
    "role_update",
    "role_delete",
    "role_policy_list",
    "role_policy_attach",
    "role_policy_detach",
    "user_role_list",
    "user_role_add",
    "user_role_remove",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_groups_that_share_a_role_name(manager).await?;
        let roles = rows(
            manager,
            Query::select()
                .columns([
                    Alias::new("id"),
                    Alias::new("tenant_id"),
                    Alias::new("name"),
                ])
                .from(Alias::new("iam_roles"))
                .to_owned(),
        )
        .await?;
        for role in roles {
            let (role_id, tenant_id) = (uuid(&role, "id")?, uuid(&role, "tenant_id")?);
            let name: String = role.try_get("", "name")?;
            let group_id = Uuid::now_v7();
            insert(
                manager,
                "iam_groups",
                ["id", "tenant_id", "name"],
                [group_id.into(), tenant_id.into(), name.into()],
            )
            .await?;
            copy_associations(
                manager,
                ("iam_user_role_attachments", "user_id"),
                ("iam_group_memberships", "user_id"),
                (role_id, group_id, tenant_id),
            )
            .await?;
            copy_associations(
                manager,
                ("iam_role_policy_attachments", "policy_id"),
                ("iam_group_policy_attachments", "policy_id"),
                (role_id, group_id, tenant_id),
            )
            .await?;
        }
        strip_removed_actions(manager).await?;
        for table in [
            "iam_user_role_attachments",
            "iam_role_policy_attachments",
            "iam_roles",
        ] {
            manager
                .drop_table(Table::drop().table(Alias::new(table)).to_owned())
                .await?;
        }
        Ok(())
    }

    /// The role tables are not restored: their data lives on in the groups.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}

async fn rows(
    manager: &SchemaManager<'_>,
    select: SelectStatement,
) -> Result<Vec<QueryResult>, DbErr> {
    manager.get_connection().query_all(&select).await
}

fn uuid(row: &QueryResult, column: &str) -> Result<Uuid, DbErr> {
    row.try_get("", column)
}

async fn insert<const N: usize>(
    manager: &SchemaManager<'_>,
    table: &str,
    columns: [&str; N],
    values: [SimpleExpr; N],
) -> Result<(), DbErr> {
    manager
        .exec_stmt(
            Query::insert()
                .into_table(Alias::new(table))
                .columns(columns.map(Alias::new))
                .values_panic(values)
                .to_owned(),
        )
        .await
}

/// A user-made group with a role's name keeps its members and gains none of the role's policies.
async fn rename_groups_that_share_a_role_name(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let colliding = rows(
        manager,
        Query::select()
            .columns([
                (Alias::new("iam_groups"), Alias::new("id")),
                (Alias::new("iam_groups"), Alias::new("name")),
            ])
            .from(Alias::new("iam_groups"))
            .inner_join(
                Alias::new("iam_roles"),
                Expr::col((Alias::new("iam_roles"), Alias::new("tenant_id")))
                    .equals((Alias::new("iam_groups"), Alias::new("tenant_id")))
                    .and(
                        Expr::col((Alias::new("iam_roles"), Alias::new("name")))
                            .equals((Alias::new("iam_groups"), Alias::new("name"))),
                    ),
            )
            .to_owned(),
    )
    .await?;
    for group in colliding {
        let id = uuid(&group, "id")?;
        let name: String = group.try_get("", "name")?;
        manager
            .exec_stmt(
                Query::update()
                    .table(Alias::new("iam_groups"))
                    .value(
                        Alias::new("name"),
                        format!("{name} (user-defined {})", &id.simple().to_string()[..8]),
                    )
                    .and_where(Expr::col(Alias::new("id")).eq(id))
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}

/// Copies one role's rows of `from` onto the new group's rows of `to`.
async fn copy_associations(
    manager: &SchemaManager<'_>,
    from: (&str, &str),
    to: (&str, &str),
    (role_id, group_id, tenant_id): (Uuid, Uuid, Uuid),
) -> Result<(), DbErr> {
    let (from_table, column) = from;
    let attachments = rows(
        manager,
        Query::select()
            .column(Alias::new(column))
            .from(Alias::new(from_table))
            .and_where(Expr::col(Alias::new("tenant_id")).eq(tenant_id))
            .and_where(Expr::col(Alias::new("role_id")).eq(role_id))
            .to_owned(),
    )
    .await?;
    for attachment in attachments {
        insert(
            manager,
            to.0,
            ["id", "tenant_id", "group_id", to.1],
            [
                Uuid::now_v7().into(),
                tenant_id.into(),
                group_id.into(),
                uuid(&attachment, column)?.into(),
            ],
        )
        .await?;
    }
    Ok(())
}

/// Removes the role actions from every stored policy document.
/// A statement left without actions is dropped, and a document left without statements cannot be valid, so its policy and attachments are deleted.
async fn strip_removed_actions(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let sqlite = manager.get_database_backend() == DbBackend::Sqlite;
    let policies = rows(
        manager,
        Query::select()
            .columns([Alias::new("id"), Alias::new("document")])
            .from(Alias::new("iam_policies"))
            .to_owned(),
    )
    .await?;
    for policy in policies {
        let id = uuid(&policy, "id")?;
        let document: serde_json::Value = if sqlite {
            let text: String = policy.try_get("", "document")?;
            serde_json::from_str(&text).map_err(|error| DbErr::Custom(error.to_string()))?
        } else {
            policy.try_get("", "document")?
        };
        let Some(statements) = document.get("statements").and_then(|s| s.as_array()) else {
            continue;
        };
        let mut changed = false;
        let kept: Vec<serde_json::Value> = statements
            .iter()
            .filter_map(|statement| {
                let mut statement = statement.clone();
                let actions = statement.get("actions")?.as_array()?.clone();
                let remaining: Vec<_> = actions
                    .iter()
                    .filter(|action| {
                        !action
                            .as_str()
                            .is_some_and(|name| REMOVED_ACTIONS.contains(&name))
                    })
                    .cloned()
                    .collect();
                changed |= remaining.len() != actions.len();
                if remaining.is_empty() {
                    return None;
                }
                statement["actions"] = serde_json::Value::Array(remaining);
                Some(statement)
            })
            .collect();
        if !changed {
            continue;
        }
        if kept.is_empty() {
            for (table, column) in [
                ("iam_user_policy_attachments", "policy_id"),
                ("iam_group_policy_attachments", "policy_id"),
                ("iam_role_policy_attachments", "policy_id"),
                ("iam_policies", "id"),
            ] {
                manager
                    .exec_stmt(
                        Query::delete()
                            .from_table(Alias::new(table))
                            .and_where(Expr::col(Alias::new(column)).eq(id))
                            .to_owned(),
                    )
                    .await?;
            }
            continue;
        }
        let updated = serde_json::json!({"statements": kept});
        manager
            .exec_stmt(
                Query::update()
                    .table(Alias::new("iam_policies"))
                    .value(Alias::new("document"), updated)
                    .and_where(Expr::col(Alias::new("id")).eq(id))
                    .to_owned(),
            )
            .await?;
    }
    Ok(())
}
