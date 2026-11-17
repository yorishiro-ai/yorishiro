//! Retires the `owner` role: stored owners become `admin`, and every tenant gains the system-managed policies and admin group that tenant creation now provides.
//!
//! PostgreSQL drops and re-adds both role checks without `owner`.
//! SQLite cannot alter a CHECK constraint without rebuilding the table, so its checks keep the legacy `owner` value; the data migration rewrites it and the enum reads it back as `admin`.
//! Raw SQL carries the data migration and the idempotent built-in inserts: the migration role owns every table, so none of this runs under tenant RLS.
//! Row ids are generated in Rust (SQLite has no `uuidv7()` column default) and inlined as literals in each backend's storage form: canonical text on PostgreSQL, `X'hex'` BLOBs on SQLite, which is what the application writes.
//! Ids are read back the same way: `::text` on PostgreSQL, `hex()` on SQLite, both parseable by `Uuid::parse_str`.

use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DatabaseExecutor, DbBackend, Statement};
use serde_json::json;
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

const ADMIN_GROUP_NAME: &str = "admins";
const ADMINISTRATOR_POLICY_NAME: &str = "Tenant Administrator";
const WRITER_POLICY_NAME: &str = "Tenant Writer";
const READER_POLICY_NAME: &str = "Tenant Reader";

// Frozen snapshot of the action catalogue at this migration; new tenants get documents generated from the current catalogue, tenants upgraded here keep this one.
const TENANT_READ_ACTIONS: &[&str] = &[
    "tenant_get",
    "workspace_list",
    "credential_list",
    "tenant_user_list",
    "group_list",
    "group_get",
    "group_member_list",
    "policy_list",
    "policy_get",
    "user_policy_list",
    "group_policy_list",
    "template_library_list",
    "template_library_get",
    "tenant_overview_get",
    "builtin_template_list",
    "builtin_template_get",
    "marketplace_list",
    "marketplace_version_list",
    "marketplace_review_list",
    "iam_audit_list",
];

const WORKSPACE_READ_ACTIONS: &[&str] = &[
    "workspace_get",
    "entity_list",
    "entity_get",
    "entity_drift_get",
    "relation_list",
    "relation_get",
    "schema_list",
    "schema_get",
    "schema_fork_list",
    "schema_fork_get",
    "schema_upstream_list",
    "schema_merge_preview",
    "search",
    "recall",
    "export",
    "import_job_get",
    "migration_dry_run",
    "migration_job_get",
    "inference_job_get",
    "inference_proposal_list",
    "inference_confirmation_get",
    "embedding_provider_get",
    "llm_provider_get",
    "worker_class_get",
    "entity_columns_get",
    "audit_log_list",
];

const ADMIN_ONLY_ACTIONS: &[&str] = &[
    "tenant_update",
    "credential_list",
    "credential_create",
    "credential_rotate",
    "credential_revoke",
    "tenant_user_list",
    "tenant_user_create",
    "tenant_user_update",
    "tenant_user_delete",
    "tenant_user_password_reset",
    "group_list",
    "group_get",
    "group_create",
    "group_update",
    "group_delete",
    "group_member_list",
    "group_member_add",
    "group_member_remove",
    "policy_list",
    "policy_get",
    "policy_create",
    "policy_update",
    "policy_delete",
    "user_policy_list",
    "group_policy_list",
    "user_policy_attach",
    "user_policy_detach",
    "group_policy_attach",
    "group_policy_detach",
    "iam_audit_list",
];

fn builtin_documents(tenant_id: Uuid) -> Vec<(&'static str, serde_json::Value)> {
    let tenant = json!({ "type": "tenant", "tenant_id": tenant_id });
    let all_workspaces = json!({ "type": "all_workspaces", "tenant_id": tenant_id });
    let both = json!([tenant.clone(), all_workspaces.clone()]);
    vec![
        (
            ADMINISTRATOR_POLICY_NAME,
            json!({
                "statements": [
                    { "effect": "allow", "actions": ["*"], "resources": both },
                ],
            }),
        ),
        (
            WRITER_POLICY_NAME,
            json!({
                "statements": [
                    { "effect": "allow", "actions": ["*"], "resources": both },
                    { "effect": "deny", "actions": ADMIN_ONLY_ACTIONS, "resources": json!([tenant]) },
                ],
            }),
        ),
        (
            READER_POLICY_NAME,
            json!({
                "statements": [
                    { "effect": "allow", "actions": TENANT_READ_ACTIONS, "resources": json!([tenant]) },
                    { "effect": "allow", "actions": WORKSPACE_READ_ACTIONS, "resources": json!([all_workspaces]) },
                ],
            }),
        ),
    ]
}

/// Standard-single-quote escaping for values inlined into a SQL literal, portable across both backends.
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

/// A row id as a SQL literal in the backend's storage form: canonical text on PostgreSQL, an `X'hex'` BLOB on SQLite.
fn id_literal(backend: DbBackend, id: Uuid) -> String {
    match backend {
        DbBackend::Postgres => format!("'{id}'"),
        _ => format!("X'{}'", id.simple()),
    }
}

/// Selects one id column and renders it as parseable text per backend: PostgreSQL casts `uuid` to its canonical text, SQLite hex-encodes the BLOB.
/// Both forms parse back through `Uuid::parse_str`.
///
/// # Errors
/// Returns the query or uuid decode failure.
async fn select_uuids(
    conn: &DatabaseExecutor<'_>,
    column: &str,
    table: &str,
    where_sql: Option<&str>,
) -> Result<Vec<Uuid>, DbErr> {
    let projection = if conn.get_database_backend() == DbBackend::Postgres {
        format!("SELECT {column}::text AS {column} FROM {table}")
    } else {
        format!("SELECT hex({column}) AS {column} FROM {table}")
    };
    let sql = match where_sql {
        Some(where_sql) => format!("{projection} WHERE {where_sql}"),
        None => projection,
    };
    let rows = conn
        .query_all_raw(Statement::from_string(conn.get_database_backend(), sql))
        .await?;
    let mut ids = Vec::with_capacity(rows.len());
    for row in rows {
        let text = row.try_get::<String>("", column)?;
        ids.push(
            text.parse::<Uuid>()
                .map_err(|error| DbErr::Custom(format!("uuid text decode failed: {error}")))?,
        );
    }
    Ok(ids)
}

/// Creates the tenant's built-in policies, the built-in group and its policy attachment, and enrolls every admin-role member in the group.
/// Idempotent: rows that already exist are left untouched, so a re-run and the application's own bootstrap stay in agreement.
///
/// # Errors
/// Returns the persistence failure, or a custom error when a just-inserted row cannot be read back.
async fn bootstrap_tenant(conn: &DatabaseExecutor<'_>, tenant_id: Uuid) -> Result<(), DbErr> {
    let backend = conn.get_database_backend();
    let tenant = id_literal(backend, tenant_id);
    for (name, document) in builtin_documents(tenant_id) {
        let document = serde_json::to_string(&document)
            .map_err(|error| DbErr::Custom(format!("builtin policy document: {error}")))?;
        let (name, document) = (escape(name), escape(&document));
        conn.execute_unprepared(&format!(
            "INSERT INTO iam_policies (id, tenant_id, name, document)
             SELECT {}, {tenant}, '{name}', '{document}'
             WHERE NOT EXISTS (
                 SELECT 1 FROM iam_policies WHERE tenant_id = {tenant} AND name = '{name}'
             )",
            id_literal(backend, Uuid::now_v7()),
        ))
        .await?;
    }
    conn.execute_unprepared(&format!(
        "INSERT INTO iam_groups (id, tenant_id, name)
         SELECT {}, {tenant}, '{group}'
         WHERE NOT EXISTS (
             SELECT 1 FROM iam_groups WHERE tenant_id = {tenant} AND name = '{group}'
         )",
        id_literal(backend, Uuid::now_v7()),
        group = escape(ADMIN_GROUP_NAME),
    ))
    .await?;
    let group_where = format!(
        "tenant_id = {tenant} AND name = '{}'",
        escape(ADMIN_GROUP_NAME)
    );
    let group_id = select_uuids(conn, "id", "iam_groups", Some(group_where.as_str()))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| DbErr::Custom("built-in group row is missing".to_owned()))?;
    let policy_where = format!(
        "tenant_id = {tenant} AND name = '{}'",
        escape(ADMINISTRATOR_POLICY_NAME)
    );
    let policy_id = select_uuids(conn, "id", "iam_policies", Some(policy_where.as_str()))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| DbErr::Custom("built-in administrator policy row is missing".to_owned()))?;
    let (group_id, policy_id) = (
        id_literal(backend, group_id),
        id_literal(backend, policy_id),
    );
    conn.execute_unprepared(&format!(
        "INSERT INTO iam_group_policy_attachments (id, tenant_id, group_id, policy_id)
         SELECT {}, {tenant}, {group_id}, {policy_id}
         WHERE NOT EXISTS (
             SELECT 1 FROM iam_group_policy_attachments
             WHERE tenant_id = {tenant} AND group_id = {group_id} AND policy_id = {policy_id}
         )",
        id_literal(backend, Uuid::now_v7()),
    ))
    .await?;
    let user_where = format!("tenant_id = {tenant} AND role = 'admin'");
    let user_ids = select_uuids(
        conn,
        "user_id",
        "tenant_memberships",
        Some(user_where.as_str()),
    )
    .await?;
    for user_id in user_ids {
        let user = id_literal(backend, user_id);
        conn.execute_unprepared(&format!(
            "INSERT INTO iam_group_memberships (id, tenant_id, group_id, user_id)
             SELECT {}, {tenant}, {group_id}, {user}
             WHERE NOT EXISTS (
                 SELECT 1 FROM iam_group_memberships
                 WHERE tenant_id = {tenant} AND group_id = {group_id} AND user_id = {user}
             )",
            id_literal(backend, Uuid::now_v7()),
        ))
        .await?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        // The update runs before the PostgreSQL checks are re-added, so the old checks still accept both spellings.
        conn.execute_unprepared(
            "UPDATE tenant_memberships SET role = 'admin' WHERE role = 'owner'",
        )
        .await?;
        conn.execute_unprepared("UPDATE workspace_invites SET role = 'admin' WHERE role = 'owner'")
            .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE tenant_memberships DROP CONSTRAINT tenant_memberships_role_check;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE tenant_memberships ADD CONSTRAINT tenant_memberships_role_check CHECK (role IN ('admin', 'member', 'viewer'));",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE workspace_invites DROP CONSTRAINT workspace_invites_role_check;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE workspace_invites ADD CONSTRAINT workspace_invites_role_check CHECK (role IN ('admin', 'member', 'viewer'));",
        )
        .await?;
        for tenant_id in select_uuids(conn, "id", "tenant_tenants", None).await? {
            bootstrap_tenant(conn, tenant_id).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Restores the legacy checks on PostgreSQL only.
        // The owner-to-admin data conversion is not reversible, the two roles are no longer distinguishable.
        // The built-in rows stay: the evaluator reads them fine, and they are inert for members without the admin role.
        helpers::pg_only(
            manager,
            "ALTER TABLE tenant_memberships DROP CONSTRAINT tenant_memberships_role_check;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE tenant_memberships ADD CONSTRAINT tenant_memberships_role_check CHECK (role IN ('owner', 'admin', 'member', 'viewer'));",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE workspace_invites DROP CONSTRAINT workspace_invites_role_check;",
        )
        .await?;
        helpers::pg_only(
            manager,
            "ALTER TABLE workspace_invites ADD CONSTRAINT workspace_invites_role_check CHECK (role IN ('owner', 'admin', 'member', 'viewer'));",
        )
        .await?;
        Ok(())
    }
}
