use super::helpers;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const AUTHENTICATORS: &str = "
CREATE OR REPLACE FUNCTION authenticate_api_key(p_key_hash bytea)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
  SELECT k.id, k.workspace_id, w.tenant_id, k.scope, k.user_id, k.audit
  FROM api_keys k JOIN workspace_workspaces w ON w.id = k.workspace_id
  WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
    AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
$$;
CREATE OR REPLACE FUNCTION authenticate_api_key(p_key_hash bytea, p_requested_workspace uuid)
RETURNS TABLE (id uuid, workspace_id uuid, tenant_id uuid, scope text, user_id uuid, audit boolean)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public, pg_temp AS $$
  SELECT k.id, COALESCE(k.workspace_id, w.id), k.tenant_id, k.scope, k.user_id, k.audit
  FROM api_keys k LEFT JOIN workspace_workspaces w
    ON k.workspace_id IS NULL AND w.id = p_requested_workspace AND w.tenant_id = k.tenant_id
  WHERE k.key_hash = p_key_hash AND k.revoked_at IS NULL
    AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)
    AND (k.workspace_id IS NOT NULL OR w.id IS NOT NULL)
$$;";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for name in ["expires_at", "revoked_at"] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("api_keys"))
                        .add_column(helpers::ts_tz_col(manager, Alias::new(name)))
                        .to_owned(),
                )
                .await?;
        }
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("api_keys"))
                    .add_column(helpers::uuid_col(manager, Alias::new("replaced_by_id")))
                    .to_owned(),
            )
            .await?;
        // SECURITY DEFINER authentication must reject inactive rows before returning any identity.
        helpers::pg_only(manager, AUTHENTICATORS).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let original = AUTHENTICATORS
            .replace(" AND k.revoked_at IS NULL", "")
            .replace(
                "\n    AND (k.expires_at IS NULL OR k.expires_at > CURRENT_TIMESTAMP)",
                "",
            );
        helpers::pg_only(manager, &original).await?;
        for name in ["replaced_by_id", "revoked_at", "expires_at"] {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("api_keys"))
                        .drop_column(Alias::new(name))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
