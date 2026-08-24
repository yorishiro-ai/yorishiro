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
                    .table(Alias::new("identity_api_key_audit_log"))
                    .if_not_exists()
                    .col(helpers::uuidv7_pk())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("tenant_id")).uuid().not_null())
                    // No foreign_key(): identity_api_keys::revoke deletes the key row outright (see its own doc comment), and a hard FK, even ON DELETE SET NULL, would still require the referenced row to exist at insert time and would touch this table on every revoke.
                    // Recorded as a plain UUID so "which key" survives the key's own deletion; a caller resolves it against identity_api_keys and reads a miss as "since revoked".
                    .col(ColumnDef::new(Alias::new("api_key_id")).uuid())
                    .col(ColumnDef::new(Alias::new("user_id")).uuid())
                    // A closed set, matching the as_db_str()/from_db_str() pattern every other stored-string enum in this codebase uses (MaintenanceMode, MembershipRole, ApiKeyScope): the CHECK constraint below is what stops a typo'd action from silently becoming a fourth value nothing filters on.
                    .col(ColumnDef::new(Alias::new("action")).text().not_null())
                    // Free-form context for the action (e.g. the migration job id an undo reverted, the maintenance mode a switch moved to and from).
                    // Not part of the closed action set: this is detail a reader inspects, not something the database branches on.
                    .col(
                        ColumnDef::new(Alias::new("detail"))
                            .json_binary()
                            .not_null(),
                    )
                    .col(helpers::created_at())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_api_key_audit_log_workspace_id")
                            .from(
                                Alias::new("identity_api_key_audit_log"),
                                Alias::new("workspace_id"),
                            )
                            .to(Alias::new("identity_workspaces"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_api_key_audit_log_tenant_id")
                            .from(
                                Alias::new("identity_api_key_audit_log"),
                                Alias::new("tenant_id"),
                            )
                            .to(Alias::new("identity_tenants"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    // SetNull, not the no-FK-at-all treatment api_key_id gets above: identity_users rows are not routinely deleted the way a revoked key is, so a real FK here is cheap, and SetNull keeps a record of "some now-deleted user did this" rather than losing the row's referential integrity.
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_identity_api_key_audit_log_user_id")
                            .from(
                                Alias::new("identity_api_key_audit_log"),
                                Alias::new("user_id"),
                            )
                            .to(Alias::new("identity_users"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .to_owned(),
            )
            .await?;

        let db = manager.get_connection();

        db.execute_unprepared(
            "ALTER TABLE identity_api_key_audit_log \
             ADD CONSTRAINT identity_api_key_audit_log_action_check \
             CHECK (action IN ('undo_migration_job', 'set_maintenance'));",
        )
        .await?;

        manager
            .create_index(
                Index::create()
                    .name("api_key_audit_log_workspace_id_created_at_idx")
                    .table(Alias::new("identity_api_key_audit_log"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("created_at"))
                    .to_owned(),
            )
            .await?;

        // Strict form, matching content_entities: yorishiro_app sets both app.current_tenant and app.current_workspace on every connection, so a write reaching this table without a workspace set is a bug, and raising surfaces that bug rather than silently writing a row nobody can read back.
        // The one write path that runs on ctx.db instead (set_maintenance, a deployment-wide operation) supplies the acting key's own tenant_id/workspace_id explicitly as column values rather than relying on the connection's GUCs, since ctx.db is the migration-role connection and carries neither.
        //
        // FORCE ROW LEVEL SECURITY is deliberately not applied: it would also bind the table owner (the migration role that performs the set_maintenance audit insert on ctx.db), and a superuser migration role bypasses FORCE regardless, so it would add nothing under test-loco's roles while silently breaking the ctx.db write path on a deployment whose migration role is a non-superuser owner.
        helpers::enable_rls_with_policy(
            db,
            "identity_api_key_audit_log",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            false,
        )
        .await?;

        // SELECT, INSERT only, deliberately, never UPDATE/DELETE: an audit trail a key can rewrite or erase isn't one.
        // yorishiro_app can append new rows and read them back, but has no way to alter or remove what has already landed.
        helpers::grant(db, "SELECT, INSERT", "identity_api_key_audit_log").await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("identity_api_key_audit_log"))
                    .to_owned(),
            )
            .await
    }
}
