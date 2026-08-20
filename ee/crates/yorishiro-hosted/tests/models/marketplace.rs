use sqlx::PgPool;
use uuid::Uuid;

use super::*;

/// Creates a template directly.
/// The marketplace only ever reads templates it did not create, so the library's own creation path is not what these tests are about.
async fn seed_template(pool: &PgPool, tenant_id: Uuid, name: &str, visibility: &str) -> Uuid {
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO identity.templates (tenant_id, name, definition, visibility) \
         VALUES ($1, $2, '{}', $3) RETURNING id",
    )
    .bind(tenant_id)
    .bind(name)
    .bind(visibility)
    .fetch_one(pool)
    .await
    .unwrap();
    row.0
}

async fn seed_tenant(pool: &PgPool, name: &str) -> Uuid {
    yorishiro_core::models::tenancy::create_tenant(pool, name, None)
        .await
        .unwrap()
        .id
}

/// Publishes a version directly against the model, bypassing `services::marketplace::publish_version`'s ownership/lock decisions: these tests are about `list_marketplace` alone, not about who may publish.
async fn seed_version(pool: &PgPool, template_id: Uuid, status: &str) {
    let mut conn = pool.acquire().await.unwrap();
    crate::models::marketplace::insert_next_version(
        &mut conn,
        template_id,
        &PublishVersionRequest {
            definition: serde_json::json!({}),
            changelog: None,
            status: status.into(),
        },
        None,
    )
    .await
    .unwrap();
}

/// A template whose only versions are drafts has nothing installable, so listing it would put an entry in the marketplace that 404s the moment anyone tries to use it.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_listing_skips_templates_with_nothing_published(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    let drafted = seed_template(&pool, tenant, "drafted", "community").await;
    let published = seed_template(&pool, tenant, "published", "community").await;

    seed_version(&pool, drafted, "draft").await;
    seed_version(&pool, published, "stable").await;

    let listing = list_marketplace(&pool, ListMarketplaceQuery::default())
        .await
        .unwrap();
    let names: Vec<_> = listing.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["published"]);
    assert_eq!(listing[0].latest_stable_version, Some(1));
}

/// A private template is not a marketplace entry, however many versions it has.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_listing_skips_private_templates(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    let private = seed_template(&pool, tenant, "private", "tenant").await;
    seed_version(&pool, private, "stable").await;

    assert!(
        list_marketplace(&pool, ListMarketplaceQuery::default())
            .await
            .unwrap()
            .is_empty()
    );
}

/// `limit`/`offset` slice the community listing rather than returning every row: with three
/// published templates and `limit: 2, offset: 1`, only the second and third (alphabetically)
/// come back.
#[sqlx::test(migrations = "../../../migrations")]
async fn listing_is_paginated(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    for name in ["alpha", "bravo", "charlie"] {
        let template = seed_template(&pool, tenant, name, "community").await;
        seed_version(&pool, template, "stable").await;
    }

    let page = list_marketplace(
        &pool,
        ListMarketplaceQuery {
            limit: 2,
            offset: 1,
        },
    )
    .await
    .unwrap();
    let names: Vec<_> = page.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["bravo", "charlie"]);
}
