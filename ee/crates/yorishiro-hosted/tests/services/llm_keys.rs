use super::*;
use crate::tests::test_helpers::{seed_tenant, seed_workspace};
use sqlx::PgPool;

/// A workspace with nothing configured must read as absent, not as an error and not as an empty
/// configuration -- the caller turns absence into a refusal, and an empty `base_url` would
/// instead produce a request to nowhere.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn an_unconfigured_workspace_has_no_credentials(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;

    assert!(get(&pool, workspace_id).await.unwrap().is_none());
    assert!(describe(&pool, workspace_id).await.unwrap().is_none());
}

/// `describe` is what an endpoint returns, so the key must not be in it. `get` is the only path
/// that yields the key, and only the inference client calls it.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn describe_reports_the_endpoint_without_the_key(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;
    set(
        &pool,
        workspace_id,
        "https://api.example.com/v1",
        "gpt-4o-mini",
        "sk-secret-value",
    )
    .await
    .unwrap();

    let described = describe(&pool, workspace_id).await.unwrap().unwrap();

    assert_eq!(described.base_url, "https://api.example.com/v1");
    assert_eq!(described.model, "gpt-4o-mini");
    assert!(described.configured);

    let rendered = serde_json::to_string(&described).unwrap();
    assert!(
        !rendered.contains("sk-secret-value"),
        "the key must not survive serialization: {rendered}"
    );
}

/// Reconfiguring replaces rather than accumulating: a workspace has one set of credentials, and
/// a second row would leave which one is used up to row order.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn setting_twice_replaces_the_credentials(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;

    set(&pool, workspace_id, "https://first/v1", "m1", "k1")
        .await
        .unwrap();
    set(&pool, workspace_id, "https://second/v1", "m2", "k2")
        .await
        .unwrap();

    let config = get(&pool, workspace_id).await.unwrap().unwrap();
    assert_eq!(config.base_url, "https://second/v1");
    assert_eq!(config.model, "m2");
    assert_eq!(config.api_key, "k2");
}

/// A trailing slash on the base URL would produce `…/v1//chat/completions`. Some providers
/// accept that and some 404, so it is normalized once here rather than at each call site.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn a_trailing_slash_is_trimmed_from_the_base_url(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;
    set(&pool, workspace_id, "https://api.example.com/v1/", "m", "k")
        .await
        .unwrap();

    assert_eq!(
        get(&pool, workspace_id).await.unwrap().unwrap().base_url,
        "https://api.example.com/v1"
    );
}

/// An empty key is a configuration mistake that would otherwise be stored and fail later, at
/// the provider, as a 401 the operator has to trace back here.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn an_empty_key_is_rejected(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;

    assert!(
        set(&pool, workspace_id, "https://x/v1", "m", "   ")
            .await
            .is_err()
    );
    assert!(get(&pool, workspace_id).await.unwrap().is_none());
}

/// Clearing makes inference refuse again, which is how a workspace turns the feature off.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn clearing_removes_the_credentials(pool: PgPool) {
    let tenant_id = seed_tenant(&pool, "t").await;
    let workspace_id = seed_workspace(&pool, tenant_id, "w").await;
    set(&pool, workspace_id, "https://x/v1", "m", "k")
        .await
        .unwrap();

    clear(&pool, workspace_id).await.unwrap();

    assert!(get(&pool, workspace_id).await.unwrap().is_none());
}

/// Both directions, because a check that only ever sees valid input proves nothing about what it
/// rejects. `file://` is the case worth naming: it would make reqwest do something that is not an
/// HTTP conversation, with a workspace's key attached.
#[test]
fn check_scheme_accepts_http_and_https_only() {
    assert!(check_scheme("https://api.openai.com/v1").is_ok());
    assert!(check_scheme("http://localhost:11434/v1").is_ok());
    assert!(check_scheme("  https://api.openai.com/v1  ").is_ok());

    for bad in [
        "file:///etc/passwd",
        "gopher://example.com",
        "api.openai.com/v1", // scheme-less: would become a relative path
        "",
    ] {
        assert!(
            check_scheme(bad).is_err(),
            "{bad:?} should have been refused"
        );
    }
}

/// `check_scheme` alone did not catch this: it trims before looking, so it accepts padding that
/// `set` was then storing verbatim. The stored value is interpolated into a request URL, so what
/// matters is what lands in the row -- which only a `set`-level test can see.
#[sqlx::test(migrator = "crate::tests::test_helpers::COMBINED_MIGRATOR")]
async fn set_stores_the_normalised_base_url(pool: PgPool) {
    let (_tenant_id, workspace_id) =
        crate::tests::test_helpers::seed_tenant_and_workspace(&pool).await;

    set(
        &pool,
        workspace_id,
        "  https://api.example.com/v1/  ",
        "gpt-4o-mini",
        "sk-test",
    )
    .await
    .unwrap();

    let described = describe(&pool, workspace_id).await.unwrap().unwrap();
    assert_eq!(described.base_url, "https://api.example.com/v1");
}
