use sqlx::PgPool;

use super::*;

/// A tenant that has never been through checkout has no billing row.
/// That is the permanent state of every self-hosted deployment, so it must read back as `None` rather than as an error.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_tenant_with_no_billing_row_reads_back_as_unbilled(pool: PgPool) {
    let tenant = yorishiro_core::models::tenancy::create_tenant(&pool, "acme", None)
        .await
        .unwrap();

    assert!(get_billing(&pool, tenant.id).await.unwrap().is_none());
}

/// Checkout can complete for a tenant that already has a row (a resubscribe after cancellation), so linking must upsert rather than fail on the primary key.
#[sqlx::test(migrations = "../../../migrations")]
async fn linking_a_customer_twice_updates_rather_than_failing(pool: PgPool) {
    let tenant = yorishiro_core::models::tenancy::create_tenant(&pool, "acme", None)
        .await
        .unwrap();

    link_stripe_customer(&pool, tenant.id, "cus_first")
        .await
        .unwrap();
    link_stripe_customer(&pool, tenant.id, "cus_second")
        .await
        .unwrap();

    let record = get_billing(&pool, tenant.id).await.unwrap().unwrap();
    assert_eq!(record.stripe_customer_id.as_deref(), Some("cus_second"));
}

/// Plan and customer id arrive on separate webhooks in whichever order Stripe delivers them, so setting one must not clear the other.
#[sqlx::test(migrations = "../../../migrations")]
async fn setting_the_plan_preserves_an_already_linked_customer(pool: PgPool) {
    let tenant = yorishiro_core::models::tenancy::create_tenant(&pool, "acme", None)
        .await
        .unwrap();

    link_stripe_customer(&pool, tenant.id, "cus_1")
        .await
        .unwrap();
    set_plan(&pool, tenant.id, "pro").await.unwrap();

    let record = get_billing(&pool, tenant.id).await.unwrap().unwrap();
    assert_eq!(record.plan.as_deref(), Some("pro"));
    assert_eq!(record.stripe_customer_id.as_deref(), Some("cus_1"));
}

/// …and in the other order: a plan set before checkout links a customer must survive the link.
#[sqlx::test(migrations = "../../../migrations")]
async fn linking_a_customer_preserves_an_already_set_plan(pool: PgPool) {
    let tenant = yorishiro_core::models::tenancy::create_tenant(&pool, "acme", None)
        .await
        .unwrap();

    set_plan(&pool, tenant.id, "team").await.unwrap();
    link_stripe_customer(&pool, tenant.id, "cus_2")
        .await
        .unwrap();

    let record = get_billing(&pool, tenant.id).await.unwrap().unwrap();
    assert_eq!(record.plan.as_deref(), Some("team"));
    assert_eq!(record.stripe_customer_id.as_deref(), Some("cus_2"));
}

/// Subscription webhooks carry only the Stripe customer id, so resolving a tenant from it is the inbound path, and an unknown customer must be `None`, not a wrong tenant.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_customer_resolves_to_its_own_tenant_and_only_its_own(pool: PgPool) {
    let a = yorishiro_core::models::tenancy::create_tenant(&pool, "a", None)
        .await
        .unwrap();
    let b = yorishiro_core::models::tenancy::create_tenant(&pool, "b", None)
        .await
        .unwrap();
    link_stripe_customer(&pool, a.id, "cus_a").await.unwrap();
    link_stripe_customer(&pool, b.id, "cus_b").await.unwrap();

    assert_eq!(
        get_by_stripe_customer(&pool, "cus_a")
            .await
            .unwrap()
            .unwrap()
            .tenant_id,
        a.id
    );
    assert_eq!(
        get_by_stripe_customer(&pool, "cus_b")
            .await
            .unwrap()
            .unwrap()
            .tenant_id,
        b.id
    );
    assert!(
        get_by_stripe_customer(&pool, "cus_unknown")
            .await
            .unwrap()
            .is_none()
    );
}
