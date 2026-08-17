//! Core domain logic for Yorishiro: the metaschema, repositories, and services that both the
//! HTTP/MCP server in this workspace and downstream crates outside it are built on.
//!
//! **This crate's `pub` API has consumers that aren't in this repository.** A `pub` item with no
//! caller in this workspace is therefore not evidence that it's dead: searching this repo can
//! only prove that *this repo* doesn't use it. Before removing or narrowing the visibility of
//! anything `pub`, check the downstream consumers too; a dead-code sweep that skips that step
//! has already come close to deleting a security-relevant function here.
//!
//! Items genuinely internal to this crate should be `pub(crate)` (or `pub(super)`) so this
//! distinction stays visible in the code rather than depending on someone remembering it.

pub mod db;
pub mod error;
pub mod metaschema;
pub mod models;
pub mod repositories;
pub mod services;
pub mod templates;

pub use error::{ResultExt, YorishiroError};

/// Shared test-only fixtures. `tenancy::create_tenant`/`create_workspace` themselves depend on
/// `PgPool` and enforce caps unrelated to what most other modules' tests need, so this crosses
/// that dependency out entirely: a minimal, direct sea-query insert against
/// `identity.tenants`/`identity.workspaces`, safe for any test module to call without pulling
/// in tenancy's cap-checking logic.
///
/// `#[cfg(test)]`-gated and `pub(crate)`: `tests/` reaches it as `crate::test_support`, since
/// every test file compiles as its own module's `mod tests` rather than as an external
/// integration test. It is therefore never part of a release build or of this crate's public
/// API.
#[cfg(test)]
pub(crate) mod test_support {
    use sea_query::{Alias, Iden, PostgresQueryBuilder, Query};
    use sea_query_binder::SqlxBinder;
    use sqlx::PgPool;
    use uuid::Uuid;

    #[derive(Iden)]
    enum Tenants {
        Table,
        Id,
        Name,
    }

    #[derive(Iden)]
    enum Workspaces {
        Table,
        Id,
        TenantId,
        Name,
    }

    pub async fn seed_tenant(pool: &PgPool, name: &str) -> Uuid {
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Tenants::Table))
            .columns([Tenants::Name])
            .values_panic([name.into()])
            .returning(Query::returning().columns([Tenants::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    pub async fn seed_workspace(pool: &PgPool, tenant_id: Uuid, name: &str) -> Uuid {
        let (sql, values) = Query::insert()
            .into_table((Alias::new("identity"), Workspaces::Table))
            .columns([Workspaces::TenantId, Workspaces::Name])
            .values_panic([tenant_id.into(), name.into()])
            .returning(Query::returning().columns([Workspaces::Id]))
            .build_sqlx(PostgresQueryBuilder);
        let (id,): (Uuid,) = sqlx::query_as_with(&sql, values)
            .fetch_one(pool)
            .await
            .unwrap();
        id
    }

    /// Seeds a tenant plus one workspace under it, returning `(tenant_id, workspace_id)`:
    /// the shape almost every test needs.
    pub async fn seed_tenant_and_workspace(pool: &PgPool) -> (Uuid, Uuid) {
        let tenant_id = seed_tenant(pool, "test-tenant").await;
        let workspace_id = seed_workspace(pool, tenant_id, "test-workspace").await;
        (tenant_id, workspace_id)
    }
}
