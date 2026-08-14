use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use yorishiro_core::YorishiroError;

use super::*;

/// Creates a template directly. The marketplace only ever reads templates it did not create, so
/// the library's own creation path is not what these tests are about.
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
    yorishiro_core::repositories::tenancy::create_tenant(pool, name, None)
        .await
        .unwrap()
        .id
}

/// A template whose only versions are drafts has nothing installable, so listing it would put an
/// entry in the marketplace that 404s the moment anyone tries to use it.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_listing_skips_templates_with_nothing_published(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    let drafted = seed_template(&pool, tenant, "drafted", "community").await;
    let published = seed_template(&pool, tenant, "published", "community").await;

    publish_version(
        &pool,
        tenant,
        drafted,
        None,
        PublishVersionRequest {
            definition: json!({}),
            changelog: None,
            status: "draft".into(),
        },
    )
    .await
    .unwrap();
    publish_version(
        &pool,
        tenant,
        published,
        None,
        PublishVersionRequest {
            definition: json!({}),
            changelog: None,
            status: "stable".into(),
        },
    )
    .await
    .unwrap();

    let listing = list_marketplace(&pool).await.unwrap();
    let names: Vec<_> = listing.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["published"]);
    assert_eq!(listing[0].latest_stable_version, Some(1));
}

/// A private template is not a marketplace entry, however many versions it has.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_listing_skips_private_templates(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    let private = seed_template(&pool, tenant, "private", "tenant").await;
    publish_version(
        &pool,
        tenant,
        private,
        None,
        PublishVersionRequest {
            definition: json!({}),
            changelog: None,
            status: "stable".into(),
        },
    )
    .await
    .unwrap();

    assert!(list_marketplace(&pool).await.unwrap().is_empty());
}

/// **The database does not enforce this** -- `template_versions` carries no RLS -- so the query
/// is the enforcement. A draft is unfinished work its owner has not chosen to show.
#[sqlx::test(migrations = "../../../migrations")]
async fn another_tenant_cannot_see_draft_versions(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    for status in ["stable", "draft"] {
        publish_version(
            &pool,
            owner,
            template,
            None,
            PublishVersionRequest {
                definition: json!({}),
                changelog: None,
                status: status.into(),
            },
        )
        .await
        .unwrap();
    }

    let seen_by_owner = list_versions(&pool, owner, template).await.unwrap();
    assert_eq!(seen_by_owner.len(), 2, "the owner sees its own draft");

    let seen_by_other = list_versions(&pool, other, template).await.unwrap();
    assert_eq!(seen_by_other.len(), 1);
    assert_eq!(seen_by_other[0].status, "stable");
}

/// Publishing is the owner's alone. Reported as NotFound so a caller cannot map out which
/// template ids exist by the difference between 403 and 404.
#[sqlx::test(migrations = "../../../migrations")]
async fn another_tenant_cannot_publish_a_version(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    let err = publish_version(
        &pool,
        other,
        template,
        None,
        PublishVersionRequest {
            definition: json!({}),
            changelog: None,
            status: "stable".into(),
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// Version numbers are assigned server-side and increment per template.
#[sqlx::test(migrations = "../../../migrations")]
async fn versions_increment_from_one(pool: PgPool) {
    let tenant = seed_tenant(&pool, "publisher").await;
    let template = seed_template(&pool, tenant, "t", "community").await;

    for expected in 1..=3 {
        let published = publish_version(
            &pool,
            tenant,
            template,
            None,
            PublishVersionRequest {
                definition: json!({}),
                changelog: None,
                status: "stable".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(published.version, expected);
    }
}

/// One review per tenant: using a template twice does not earn a second vote.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_second_review_replaces_the_first(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let reviewer = seed_tenant(&pool, "reviewer").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    for rating in [5, 2] {
        submit_review(
            &pool,
            reviewer,
            template,
            None,
            SubmitReviewRequest {
                rating,
                comment: None,
            },
        )
        .await
        .unwrap();
    }

    let reviews = list_reviews(&pool, owner, template).await.unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].rating, 2);
}

#[sqlx::test(migrations = "../../../migrations")]
async fn a_rating_outside_one_to_five_is_rejected(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    for rating in [0, 6] {
        let err = submit_review(
            &pool,
            owner,
            template,
            None,
            SubmitReviewRequest {
                rating,
                comment: None,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, YorishiroError::ValidationFailed { .. }),
            "rating {rating}"
        );
    }
}

/// Reviewing a template the caller cannot see would confirm that it exists.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_private_template_cannot_be_reviewed_by_another_tenant(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "private", "tenant").await;

    let err = submit_review(
        &pool,
        other,
        template,
        None,
        SubmitReviewRequest {
            rating: 5,
            comment: None,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

/// A fork takes the chosen *version's* definition, and lands private in the caller's own library.
#[sqlx::test(migrations = "../../../migrations")]
async fn forking_copies_the_published_version_into_the_callers_tenant(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    publish_version(
        &pool,
        owner,
        template,
        None,
        PublishVersionRequest {
            definition: json!({ "marker": "v1" }),
            changelog: None,
            status: "stable".into(),
        },
    )
    .await
    .unwrap();

    let forked = fork_template(&pool, other, template, None, None)
        .await
        .unwrap();

    let row: (Uuid, String, serde_json::Value, Option<Uuid>) = sqlx::query_as(
        "SELECT tenant_id, visibility, definition, fork_of FROM identity.templates WHERE id = $1",
    )
    .bind(forked)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, other, "the fork belongs to the forking tenant");
    assert_eq!(row.1, "tenant", "a fork starts private, not republished");
    assert_eq!(row.2["marker"], "v1");
    assert_eq!(row.3, Some(template));
}

/// A draft is not published, so it cannot be forked even by naming its version number.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_draft_version_cannot_be_forked(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "shared", "community").await;

    publish_version(
        &pool,
        owner,
        template,
        None,
        PublishVersionRequest {
            definition: json!({}),
            changelog: None,
            status: "draft".into(),
        },
    )
    .await
    .unwrap();

    let err = fork_template(&pool, other, template, Some(1), None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../../migrations")]
async fn only_the_owner_can_change_visibility(pool: PgPool) {
    let owner = seed_tenant(&pool, "owner").await;
    let other = seed_tenant(&pool, "other").await;
    let template = seed_template(&pool, owner, "t", "tenant").await;

    let err = set_visibility(&pool, other, template, "community")
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));

    set_visibility(&pool, owner, template, "community")
        .await
        .unwrap();
    let row: (String,) = sqlx::query_as("SELECT visibility FROM identity.templates WHERE id = $1")
        .bind(template)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.0, "community");
}

/// The version number is read by `max(version) + 1` inside the statement that inserts it, and at
/// READ COMMITTED Postgres locks no range for rows that do not exist yet. Without the advisory
/// lock in `publish_version`, two concurrent publishes of one template both read the same
/// maximum, both write the same number, and `UNIQUE (template_id, version)` fails one of them --
/// an opaque 500 for a caller that did nothing wrong.
///
/// All of them must succeed, taking consecutive numbers.
///
/// Two details make this an actual check rather than a passing assertion:
///
/// * The publishes are **spawned**, not `tokio::join!`ed. `join!` drives every future from one
///   task, so they interleave only at await points and each read lands after the previous
///   insert -- the race never occurs and the test proves nothing. Separate tasks released by a
///   barrier put them inside the read window together.
/// * There are **eight**, not two. With two, removing the advisory lock failed this only about
///   one run in three: the window is narrow, and a test that catches a bug a third of the time
///   is a flaky test rather than a gate. Eight widens it enough that the unguarded version
///   fails every time, measured before this was relied on.
#[sqlx::test(migrations = "../../../migrations")]
async fn concurrent_publishes_get_consecutive_versions(pool: PgPool) {
    const PUBLISHERS: usize = 8;

    let tenant = seed_tenant(&pool, "publisher").await;
    let template = seed_template(&pool, tenant, "t", "community").await;

    let gate = std::sync::Arc::new(tokio::sync::Barrier::new(PUBLISHERS));
    let mut handles = Vec::new();
    for _ in 0..PUBLISHERS {
        let pool = pool.clone();
        let gate = gate.clone();
        handles.push(tokio::spawn(async move {
            gate.wait().await;
            publish_version(
                &pool,
                tenant,
                template,
                None,
                PublishVersionRequest {
                    definition: json!({}),
                    changelog: None,
                    status: "stable".into(),
                },
            )
            .await
        }));
    }

    let mut versions = Vec::new();
    for handle in handles {
        let published = handle
            .await
            .expect("the publish task must not panic")
            .expect("both concurrent publishes must succeed");
        versions.push(published.version);
    }
    versions.sort_unstable();
    assert_eq!(
        versions,
        (1..=PUBLISHERS as i32).collect::<Vec<_>>(),
        "concurrent publishes must take distinct, consecutive versions"
    );
}
