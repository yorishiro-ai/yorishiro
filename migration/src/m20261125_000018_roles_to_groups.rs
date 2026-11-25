//! Replaces IAM roles with groups.
//!
//! Each role becomes a new group of the same name, keeping its members and attached policies.
//! A group that already has the role's name is a different principal with different members, so it is renamed first.
//! Merging would hand the role's policies to that group's members.
//! Stored policy documents lose the removed role actions, so every tenant keeps documents the application can parse.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

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
        // SQLite has no column default for ids; a random 16-byte value is a valid UUID blob.
        let (id_column, id_value) = if manager.get_database_backend() == DbBackend::Sqlite {
            ("id, ", "randomblob(16), ")
        } else {
            ("", "")
        };
        let db = manager.get_connection();
        let suffix = if manager.get_database_backend() == DbBackend::Sqlite {
            "lower(substr(hex(g.id), 1, 8))"
        } else {
            "substr(g.id::text, 1, 8)"
        };
        db.execute_unprepared(&format!(
            "UPDATE iam_groups SET name = name || ' (user-defined ' || (SELECT {suffix} FROM iam_groups g WHERE g.id = iam_groups.id) || ')'
             WHERE EXISTS (SELECT 1 FROM iam_roles r WHERE r.tenant_id = iam_groups.tenant_id AND r.name = iam_groups.name)"
        ))
        .await?;
        db.execute_unprepared(&format!(
            "INSERT INTO iam_groups ({id_column}tenant_id, name)
             SELECT {id_value}r.tenant_id, r.name FROM iam_roles r
             WHERE NOT EXISTS (SELECT 1 FROM iam_groups g WHERE g.tenant_id = r.tenant_id AND g.name = r.name)"
        ))
        .await?;
        db.execute_unprepared(&format!(
            "INSERT INTO iam_group_memberships ({id_column}tenant_id, group_id, user_id)
             SELECT {id_value}a.tenant_id, g.id, a.user_id
             FROM iam_user_role_attachments a
             JOIN iam_roles r ON r.tenant_id = a.tenant_id AND r.id = a.role_id
             JOIN iam_groups g ON g.tenant_id = r.tenant_id AND g.name = r.name
             WHERE NOT EXISTS (SELECT 1 FROM iam_group_memberships m
                 WHERE m.tenant_id = a.tenant_id AND m.group_id = g.id AND m.user_id = a.user_id)"
        ))
        .await?;
        db.execute_unprepared(&format!(
            "INSERT INTO iam_group_policy_attachments ({id_column}tenant_id, group_id, policy_id)
             SELECT {id_value}a.tenant_id, g.id, a.policy_id
             FROM iam_role_policy_attachments a
             JOIN iam_roles r ON r.tenant_id = a.tenant_id AND r.id = a.role_id
             JOIN iam_groups g ON g.tenant_id = r.tenant_id AND g.name = r.name
             WHERE NOT EXISTS (SELECT 1 FROM iam_group_policy_attachments p
                 WHERE p.tenant_id = a.tenant_id AND p.group_id = g.id AND p.policy_id = a.policy_id)"
        ))
        .await?;
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

/// Removes the role actions from every stored policy document.
/// A statement left without actions is dropped, and a document left without statements cannot be valid, so its policy and attachments are deleted.
async fn strip_removed_actions(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let db = manager.get_connection();
    let backend = manager.get_database_backend();
    let sqlite = backend == DbBackend::Sqlite;
    let (p1, p2) = if sqlite { ("?", "?") } else { ("$1", "$2") };
    let rows = db
        .query_all_raw(Statement::from_string(
            backend,
            "SELECT id, document FROM iam_policies",
        ))
        .await?;
    for row in rows {
        let id: uuid::Uuid = row.try_get("", "id")?;
        let document: serde_json::Value = if backend == DbBackend::Sqlite {
            let text: String = row.try_get("", "document")?;
            serde_json::from_str(&text).map_err(|error| DbErr::Custom(error.to_string()))?
        } else {
            row.try_get("", "document")?
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
                if remaining.len() != actions.len() {
                    changed = true;
                }
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
            for sql in [
                format!("DELETE FROM iam_user_policy_attachments WHERE policy_id = {p1}"),
                format!("DELETE FROM iam_group_policy_attachments WHERE policy_id = {p1}"),
                format!("DELETE FROM iam_role_policy_attachments WHERE policy_id = {p1}"),
                format!("DELETE FROM iam_policies WHERE id = {p1}"),
            ] {
                db.execute_raw(Statement::from_sql_and_values(
                    backend,
                    sql.as_str(),
                    [Value::from(id)],
                ))
                .await?;
            }
            continue;
        }
        let updated = serde_json::json!({"statements": kept}).to_string();
        let sql = if sqlite {
            format!("UPDATE iam_policies SET document = {p1} WHERE id = {p2}")
        } else {
            format!("UPDATE iam_policies SET document = {p1}::jsonb WHERE id = {p2}")
        };
        db.execute_raw(Statement::from_sql_and_values(
            backend,
            sql.as_str(),
            [Value::from(updated), Value::from(id)],
        ))
        .await?;
    }
    Ok(())
}
