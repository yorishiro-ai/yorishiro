use sqlx::PgPool;

use super::*;

/// Every built-in template ends up listed, owned by the official tenant, with a stable version.
#[sqlx::test(migrations = "../../../migrations")]
async fn seeding_publishes_every_builtin_template(pool: PgPool) {
    let outcome = seed_official_templates(&pool).await.unwrap();

    let builtin_count = yorishiro_core::templates::list_templates().len();
    assert_eq!(outcome.published.len(), builtin_count);
    assert!(outcome.updated.is_empty());

    let listings = crate::services::marketplace::list_marketplace(&pool)
        .await
        .unwrap();
    assert_eq!(listings.len(), builtin_count);
    for listing in &listings {
        assert_eq!(listing.tenant_id, OFFICIAL_TENANT_ID);
        assert_eq!(listing.author.as_deref(), Some(OFFICIAL_AUTHOR));
        // A listing with no stable version cannot be installed.
        assert_eq!(listing.latest_stable_version, Some(1));
    }
}

/// **The property that lets this run on every deployment.** A second run with unchanged
/// built-ins must not publish a second version, or the version number climbs forever and every
/// listing claims an update nobody made.
#[sqlx::test(migrations = "../../../migrations")]
async fn seeding_twice_changes_nothing_the_second_time(pool: PgPool) {
    seed_official_templates(&pool).await.unwrap();
    let second = seed_official_templates(&pool).await.unwrap();

    assert!(second.published.is_empty());
    assert!(second.updated.is_empty());
    assert_eq!(
        second.unchanged.len(),
        yorishiro_core::templates::list_templates().len()
    );

    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM identity.template_versions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        yorishiro_core::templates::list_templates().len() as i64
    );
}

/// A built-in whose definition changed between releases publishes a new version rather than
/// silently editing the one tenants already installed.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_changed_builtin_publishes_a_new_version(pool: PgPool) {
    seed_official_templates(&pool).await.unwrap();

    // Stand in for "the built-in changed upstream" by rewriting what was published.
    sqlx::query("UPDATE identity.template_versions SET definition = '{\"stale\": true}'")
        .execute(&pool)
        .await
        .unwrap();

    let outcome = seed_official_templates(&pool).await.unwrap();
    assert_eq!(
        outcome.updated.len(),
        yorishiro_core::templates::list_templates().len()
    );

    let max_version: (i32,) = sqlx::query_as("SELECT max(version) FROM identity.template_versions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(max_version.0, 2);
}

/// The publisher exists only to own the listings: it must not be a tenant anyone can use, or
/// the marketplace would ship with an account nobody controls but everyone can reach.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_official_tenant_has_no_members_and_no_workspaces(pool: PgPool) {
    seed_official_templates(&pool).await.unwrap();

    let members: (i64,) =
        sqlx::query_as("SELECT count(*) FROM identity.tenant_memberships WHERE tenant_id = $1")
            .bind(OFFICIAL_TENANT_ID)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(members.0, 0);

    let workspaces: (i64,) =
        sqlx::query_as("SELECT count(*) FROM identity.workspaces WHERE tenant_id = $1")
            .bind(OFFICIAL_TENANT_ID)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(workspaces.0, 0);
}
