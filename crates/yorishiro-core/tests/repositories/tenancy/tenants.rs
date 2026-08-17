use sqlx::PgPool;

use crate::YorishiroError;
use crate::repositories::tenancy::{create_tenant_with_cap, max_tenants_from_env};

#[sqlx::test(migrations = "../../migrations")]
async fn enforces_system_wide_tenant_cap(pool: PgPool) {
    create_tenant_with_cap(&pool, "first", None, Some(1))
        .await
        .unwrap();

    let err = create_tenant_with_cap(&pool, "second", None, Some(1))
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn unset_tenant_cap_is_unlimited(pool: PgPool) {
    create_tenant_with_cap(&pool, "first", None, None)
        .await
        .unwrap();
    create_tenant_with_cap(&pool, "second", None, None)
        .await
        .unwrap();
}

/// `YORISHIRO_MAX_TENANTS` is process-wide state, so these tests serialize through this lock rather than racing the env var against each other.
static MAX_TENANTS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Sets the variable for one test and removes it on drop, including when the test panics.
///
/// The previous version cleared it by hand after each assertion.
/// That happened to be safe (every test read its result into a local before asserting), but it was one reordered line away from leaking a value into whichever test ran next.
/// `Drop` makes the cleanup a property of the guard rather than of each test body's shape, matching `yorishiro-server`'s `tests/config/mod.rs`.
///
/// The lock is taken with `unwrap_or_else(|e| e.into_inner())`: a test that panics while holding it poisons the mutex, and a plain `unwrap()` would turn one real failure into a cascade of unrelated ones.
/// This guard's `Drop` restores the variable either way, so the state behind the poisoned lock is not actually suspect.
struct MaxTenantsGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl MaxTenantsGuard {
    fn set(value: Option<&str>) -> Self {
        let lock = MAX_TENANTS_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: serialized by MAX_TENANTS_ENV_LOCK, and nothing else touches this key.
        unsafe {
            match value {
                Some(v) => std::env::set_var("YORISHIRO_MAX_TENANTS", v),
                None => std::env::remove_var("YORISHIRO_MAX_TENANTS"),
            }
        }
        Self { _lock: lock }
    }
}

impl Drop for MaxTenantsGuard {
    fn drop(&mut self) {
        // SAFETY: serialized by MAX_TENANTS_ENV_LOCK, and nothing else touches this key.
        unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    }
}

#[test]
fn max_tenants_from_env_unset_is_unlimited() {
    let _guard = MaxTenantsGuard::set(None);
    assert_eq!(max_tenants_from_env().unwrap(), None);
}

/// Zero means "no cap", not "no tenant may be created": an operator writing `0` to turn the limit off must not lock themselves out of creating any tenant at all.
#[test]
fn max_tenants_from_env_zero_is_unlimited() {
    let _guard = MaxTenantsGuard::set(Some("0"));
    assert_eq!(max_tenants_from_env().unwrap(), None);
}

#[test]
fn max_tenants_from_env_positive_is_the_cap() {
    let _guard = MaxTenantsGuard::set(Some("3"));
    assert_eq!(max_tenants_from_env().unwrap(), Some(3));
}

/// A negative or non-numeric value is a typo in the deployment config.
/// It has to fail loudly rather than read as "unlimited", which would silently remove the cap an operator believed they had set.
#[test]
fn max_tenants_from_env_rejects_negative() {
    let _guard = MaxTenantsGuard::set(Some("-1"));
    assert!(max_tenants_from_env().is_err());
}

#[test]
fn max_tenants_from_env_rejects_non_integer() {
    let _guard = MaxTenantsGuard::set(Some("abc"));
    assert!(max_tenants_from_env().is_err());
}
