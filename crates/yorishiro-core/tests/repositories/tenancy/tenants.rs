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

/// `YORISHIRO_MAX_TENANTS` is process-wide state, so these tests serialize through this lock
/// rather than racing the env var against each other.
static MAX_TENANTS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn max_tenants_from_env_unset_is_unlimited() {
    let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
    unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    assert_eq!(max_tenants_from_env().unwrap(), None);
}

#[test]
fn max_tenants_from_env_zero_is_unlimited() {
    let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "0") };
    let result = max_tenants_from_env().unwrap();
    unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    assert_eq!(result, None);
}

#[test]
fn max_tenants_from_env_positive_is_the_cap() {
    let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "3") };
    let result = max_tenants_from_env().unwrap();
    unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    assert_eq!(result, Some(3));
}

#[test]
fn max_tenants_from_env_rejects_negative() {
    let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "-1") };
    let result = max_tenants_from_env();
    unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    assert!(result.is_err());
}

#[test]
fn max_tenants_from_env_rejects_non_integer() {
    let _guard = MAX_TENANTS_ENV_LOCK.lock().unwrap();
    unsafe { std::env::set_var("YORISHIRO_MAX_TENANTS", "abc") };
    let result = max_tenants_from_env();
    unsafe { std::env::remove_var("YORISHIRO_MAX_TENANTS") };
    assert!(result.is_err());
}
