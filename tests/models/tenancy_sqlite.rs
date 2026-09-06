use migration::{Migrator, MigratorTrait};
/// SQLite-specific tests for tenancy: single-tenant cap and invite ID generation.
use sea_orm::Database;
use serial_test::serial;
use yorishiro::error::YorishiroError;
use yorishiro::models::tenancy::{MembershipRole, create_invite, create_tenant};

/// A fresh in-memory SQLite database, migrated.
/// Each test gets its own, so nothing but the process-wide `YORISHIRO_MAX_TENANTS` env var is shared between them.
async fn sqlite_db() -> sea_orm::DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("connect to in-memory sqlite");
    Migrator::up(&db, None).await.expect("run migrations");
    db
}

/// Sets `YORISHIRO_MAX_TENANTS` for the duration of `fut`, restoring whatever was there before.
/// Callers must be `#[serial]`: this mutates process-wide state, and `create_tenant`'s own SQLite path ignores it regardless, but the Postgres branch inside `create_tenant` still reads it, so a concurrent test observing an unexpected value would be a real (if here unlikely) source of flakiness.
async fn with_max_tenants<T>(value: &str, fut: impl std::future::Future<Output = T>) -> T {
    let previous = std::env::var("YORISHIRO_MAX_TENANTS").ok();
    // SAFETY: serialized by every test that touches this env var being #[serial] on the default key.
    unsafe {
        std::env::set_var("YORISHIRO_MAX_TENANTS", value);
    }
    let result = fut.await;
    unsafe {
        match &previous {
            Some(v) => std::env::set_var("YORISHIRO_MAX_TENANTS", v),
            None => std::env::remove_var("YORISHIRO_MAX_TENANTS"),
        }
    }
    result
}

#[tokio::test]
#[serial]
async fn a_first_tenant_can_be_created_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let db = sqlite_db().await;
    let tenant = create_tenant(&db, "first tenant")
        .await
        .expect("first tenant should be created");
    assert_eq!(tenant.name, "first tenant");
}

#[tokio::test]
#[serial]
async fn a_second_tenant_is_refused_on_sqlite_even_with_a_large_max_tenants() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let db = sqlite_db().await;
    // A generous limit: if SQLite's cap were reading this instead of being hardcoded to 1, the second create below would wrongly succeed.
    with_max_tenants("1000", async {
        create_tenant(&db, "first tenant")
            .await
            .expect("first tenant should be created");

        let err = create_tenant(&db, "second tenant")
            .await
            .expect_err("a second tenant must be refused on sqlite");
        assert!(
            matches!(err, YorishiroError::Conflict { .. }),
            "expected Conflict, got {err:?}"
        );
    })
    .await;
}

/// `create_invite` builds its `ActiveModel` with `..Default::default()`, so `id` reaches the insert `NotSet`.
/// PostgreSQL fills it from the column's `uuidv7()` default; SQLite has no such default, so this table's `before_save` must call `db::sqlite_generated_id` or the insert fails with `NOT NULL constraint failed: identity_invites.id`.
///
/// This test exists here rather than in `tests/` because that suite is PostgreSQL-only (`request_with_create_db` issues `CREATE DATABASE`), so nothing there reaches the backend where the failure occurs.
#[tokio::test]
#[serial]
async fn an_invite_gets_an_id_on_sqlite() {
    if !super::super::require_sqlite_backend() {
        return;
    }
    let db = sqlite_db().await;
    let tenant = create_tenant(&db, "invite tenant")
        .await
        .expect("create tenant");

    let (invite, _token) = create_invite(
        &db,
        tenant.id,
        "invitee@example.com",
        MembershipRole::Member,
        chrono::Duration::days(7),
    )
    .await
    .expect("create invite on sqlite");

    assert!(
        !invite.id.is_nil(),
        "the invite must carry a generated id, not a nil UUID"
    );
}
