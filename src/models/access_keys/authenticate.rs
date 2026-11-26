//! Resolving a presented key to its workspace and tenant, and recording its use.

use super::{
    AuthContext, ColumnTrait, ConnectionTrait, DbHandle, Entity, EntityTrait, Expr,
    FromQueryResult, Model, QueryFilter, ResultExt, Uuid, WORKSPACE_HEADER, YorishiroError,
    hash_key,
};

impl Entity {
    /// Verifies a presented raw access key and resolves the workspace and tenant it belongs to.
    ///
    /// At this point neither the workspace nor the tenant is known yet, so RLS's `app.current_workspace`/`app.current_tenant` can't be set, which is why this takes the whole [`DbHandle`] rather than a scoped connection.
    ///
    /// `SeaQuery` calls the `authenticate_access_key` SECURITY DEFINER function to read through RLS and projects only `id/workspace_id/tenant_id/user_id/audit`, never `key_hash` itself.
    ///
    /// Non-root workspace-scoped keys resolve to their recorded workspace.
    /// A tenant-scoped key resolves `requested_workspace` when present, otherwise it has no workspace.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn authenticate(
        db: &DbHandle,
        presented_key: &str,
        requested_workspace: Option<Uuid>,
    ) -> Result<AuthContext, YorishiroError> {
        use sea_orm::sea_query::{Alias, Expr, Func, Query};
        #[derive(sea_orm::FromQueryResult)]
        struct Credential {
            id: Uuid,
            tenant_id: Uuid,
            user_id: Option<Uuid>,
        }
        let query = Query::select()
            .columns(["id", "tenant_id", "user_id"].map(Alias::new))
            .from_function(
                Func::cust(Alias::new("authenticate_access_key"))
                    .arg(Expr::val(hash_key(presented_key))),
                Alias::new("credential"),
            )
            .to_owned();
        let conn =
            sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(db.tenant.pool().clone());
        let row = Credential::find_by_statement(conn.get_database_backend().build(&query))
            .one(&conn)
            .await
            .internal()?;

        let row = row.ok_or(YorishiroError::Unauthenticated)?;
        let workspace_id = Self::resolve_key_workspace(
            &db.identity_connection(),
            row.tenant_id,
            requested_workspace,
        )
        .await?;
        AuthContext {
            access_key_id: row.id,
            workspace_id,
            tenant_id: row.tenant_id,
            user_id: row.user_id,
        }
        .with_current_membership(&db.identity_connection())
        .await
    }

    /// Chooses which workspace a verified credential acts on: the requested one, when it belongs to the credential's tenant.
    ///
    /// # Errors
    /// Rejects a workspace of another tenant.
    async fn resolve_key_workspace(
        conn: &impl ConnectionTrait,
        tenant_id: Uuid,
        requested: Option<Uuid>,
    ) -> Result<Option<Uuid>, YorishiroError> {
        if let Some(requested) = requested {
            let tenant =
                crate::models::workspace_workspaces::get_workspace_tenant(conn, requested).await?;
            if tenant != tenant_id {
                // The key simply cannot act on another tenant's workspace; report it as a scope denial so a well-formed request is not logged as a validation failure.
                return Err(YorishiroError::AccessDenied {
                    message: format!("{WORKSPACE_HEADER} names a workspace this key cannot act on"),
                    hint: "the named workspace belongs to another tenant; send a workspace this \
                            key can act on"
                        .into(),
                });
            }
            return Ok(Some(requested));
        }
        Ok(None)
    }

    /// SQLite equivalent of [`Self::authenticate`], for a deployment with no `DbHandle` (see `Hooks::after_context`).
    ///
    /// The Postgres path goes through the SECURITY DEFINER function specifically to read rows RLS would otherwise hide from an unauthenticated caller; SQLite has no RLS at all, so there is nothing to bypass, and this queries the tables directly with the `SeaORM` entity API.
    /// Workspace resolution follows the same rules as the Postgres path.
    ///
    /// The default authenticator selects this implementation for SQLite.
    ///
    /// # Errors
    /// Returns an error if the operation cannot be completed.
    pub async fn authenticate_sqlite(
        conn: &impl ConnectionTrait,
        presented_key: &str,
        requested_workspace: Option<Uuid>,
    ) -> Result<AuthContext, YorishiroError> {
        let key = Self::find_sqlite_auth_context(conn, hash_key(presented_key))
            .await?
            .ok_or(YorishiroError::Unauthenticated)?;
        let workspace_id =
            Self::resolve_key_workspace(conn, key.tenant_id, requested_workspace).await?;
        AuthContext {
            access_key_id: key.id,
            workspace_id,
            tenant_id: key.tenant_id,
            user_id: key.user_id,
        }
        .with_current_membership(conn)
        .await
    }

    /// Records authenticated usage outside the request transaction, preserving read-only requests' timestamps.
    /// Connection failures are best-effort on both backends and never reject a valid credential.
    pub(crate) async fn touch_authenticated(
        app: &loco_rs::app::AppContext,
        auth: &AuthContext,
    ) -> Result<(), YorishiroError> {
        if crate::db::is_sqlite(&app.db) {
            Self::touch_last_used(&app.db, auth.access_key_id).await;
            return Ok(());
        }
        let db = DbHandle::of(app)?;
        let connection = match auth.workspace_id {
            Some(workspace_id) => {
                db.tenant
                    .acquire_for_workspace(auth.tenant_id, workspace_id)
                    .await
            }
            None => db.tenant.acquire_for_tenant(auth.tenant_id).await,
        };
        match connection {
            Ok(mut conn) => {
                if let Err(error) =
                    Self::touch_last_used_on_connection(conn.as_mut(), auth.access_key_id).await
                {
                    tracing::warn!(%error, "failed to update access key last_used_at");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to acquire connection to touch last_used_at");
            }
        }
        Ok(())
    }

    /// Records the key's last-used timestamp on a raw `sqlx` connection.
    /// Best-effort: doesn't affect authentication outcomes, so callers don't need to fail the whole request if it errors.
    ///
    /// Deliberately not run on the request's `DatabaseTransaction`: a read-only handler drops that transaction without committing, which would silently roll this update back along with it.
    /// Every caller uses a short-lived connection from `TenantDb::acquire_for_workspace` instead.
    ///
    /// This call receives a bare RLS-scoped `PgConnection`, not a `SeaORM` `ConnectionTrait`, so the write must use `sqlx` on that connection.
    pub(crate) async fn touch_last_used_on_connection(
        conn: &mut sea_orm::sqlx::PgConnection,
        access_key_id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::access_keys::Column;
        use sea_orm::sea_query::{Expr, ExprTrait, Func, PostgresQueryBuilder, Query};
        let (sql, _) = Query::update()
            .table(Self)
            .value(
                Column::LastUsedAt,
                Func::cust(sea_orm::sea_query::Alias::new("now")),
            )
            .and_where(Expr::col(Column::Id).eq(access_key_id))
            .build(PostgresQueryBuilder);
        sea_orm::sqlx::query(sea_orm::sqlx::AssertSqlSafe(sql))
            .bind(access_key_id)
            .execute(conn)
            .await
            .internal()?;
        Ok(())
    }

    pub(crate) async fn find_sqlite_auth_context(
        conn: &impl ConnectionTrait,
        key_hash: Vec<u8>,
    ) -> Result<Option<Model>, YorishiroError> {
        use crate::models::_entities::access_keys::Column;

        let Some(key) = Self::find()
            .filter(Column::KeyHash.eq(key_hash))
            .one(conn)
            .await
            .internal()?
        else {
            return Ok(None);
        };
        if !key.is_active() {
            return Ok(None);
        }
        Ok(Some(key))
    }

    pub(crate) async fn touch_last_used(conn: &impl ConnectionTrait, access_key_id: Uuid) {
        use crate::models::_entities::access_keys::Column;

        if let Err(err) = Self::update_many()
            .col_expr(Column::LastUsedAt, Expr::value(crate::data::time::now()))
            .filter(Column::Id.eq(access_key_id))
            .exec(conn)
            .await
        {
            tracing::warn!(error = %err, "failed to update access key last_used_at");
        }
    }
}
