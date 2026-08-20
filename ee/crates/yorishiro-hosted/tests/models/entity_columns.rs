use super::*;
use crate::tests::test_helpers::seed_tenant_and_workspace;
use sqlx::PgPool;

/// A workspace that has never chosen must read as absent, not as an empty selection.
/// The caller turns absence into "derive columns from the schema" and an empty list into "the workspace chose to show nothing", so collapsing the two would make a reset render a table with no columns.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_workspace_that_never_chose_has_no_preference(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    assert!(get(&mut conn, workspace_id, "task").await.unwrap().is_none());
    assert!(list(&mut conn, workspace_id).await.unwrap().is_empty());
}

/// The stored order is the display order, so it must survive the round trip exactly rather than coming back sorted or deduplicated.
#[sqlx::test(migrations = "../../../migrations")]
async fn the_stored_order_is_the_order_read_back(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let chosen = vec!["priority".to_string(), "title".to_string(), "done".to_string()];
    set(&mut conn, workspace_id, "task", &chosen).await.unwrap();

    let stored = get(&mut conn, workspace_id, "task").await.unwrap().unwrap();
    assert_eq!(stored.columns, chosen);
    assert_eq!(stored.entity_type, "task");
}

/// Saving twice must leave one row, not two.
/// The unique constraint is what makes the second save an update; without it a concurrent pair of saves would insert twice and a reader would pick one arbitrarily.
#[sqlx::test(migrations = "../../../migrations")]
async fn saving_again_replaces_rather_than_appends(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    set(&mut conn, workspace_id, "task", &["title".to_string()])
        .await
        .unwrap();
    set(&mut conn, workspace_id, "task", &["done".to_string(), "title".to_string()])
        .await
        .unwrap();

    let all = list(&mut conn, workspace_id).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].columns, vec!["done".to_string(), "title".to_string()]);
}

/// Two entity types in one workspace have different fields, so they must not share a row.
#[sqlx::test(migrations = "../../../migrations")]
async fn each_entity_type_keeps_its_own_columns(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    set(&mut conn, workspace_id, "task", &["title".to_string()])
        .await
        .unwrap();
    set(&mut conn, workspace_id, "note", &["body".to_string()])
        .await
        .unwrap();

    assert_eq!(
        get(&mut conn, workspace_id, "task").await.unwrap().unwrap().columns,
        vec!["title".to_string()]
    );
    assert_eq!(
        get(&mut conn, workspace_id, "note").await.unwrap().unwrap().columns,
        vec!["body".to_string()]
    );
    assert_eq!(list(&mut conn, workspace_id).await.unwrap().len(), 2);
}

/// Clearing must remove the row, not store an empty list.
/// An empty list means "show nothing", and a reset has to be distinguishable from it, which is only true if absence comes back.
#[sqlx::test(migrations = "../../../migrations")]
async fn clearing_restores_absence_rather_than_storing_emptiness(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    set(&mut conn, workspace_id, "task", &["title".to_string()])
        .await
        .unwrap();
    clear(&mut conn, workspace_id, "task").await.unwrap();

    assert!(get(&mut conn, workspace_id, "task").await.unwrap().is_none());
}

/// An explicit empty selection is a choice and must be stored as one, distinct from never having chosen.
#[sqlx::test(migrations = "../../../migrations")]
async fn an_empty_selection_is_stored_not_treated_as_absent(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    set(&mut conn, workspace_id, "task", &[]).await.unwrap();

    let stored = get(&mut conn, workspace_id, "task").await.unwrap();
    assert!(stored.is_some(), "an empty choice must still be a row");
    assert!(stored.unwrap().columns.is_empty());
}

/// A duplicate would render the same field twice and make reordering ambiguous, so it is refused at the boundary rather than deduplicated silently.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_repeated_column_is_refused(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let err = set(
        &mut conn,
        workspace_id,
        "task",
        &["title".to_string(), "title".to_string()],
    )
    .await
    .unwrap_err();

    assert!(
        format!("{err:?}").contains("more than once"),
        "expected a duplicate-column refusal, got {err:?}"
    );
    assert!(
        get(&mut conn, workspace_id, "task").await.unwrap().is_none(),
        "a refused save must not leave a row behind"
    );
}

/// A table wider than the screen stops being a table, so the count is bounded.
/// Asserted at the boundary rather than against the constant's value: what matters is that one more than the maximum is refused.
#[sqlx::test(migrations = "../../../migrations")]
async fn more_columns_than_the_maximum_are_refused(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let at_limit: Vec<String> = (0..MAX_VISIBLE_COLUMNS).map(|i| format!("f{i}")).collect();
    set(&mut conn, workspace_id, "task", &at_limit).await.unwrap();

    let over: Vec<String> = (0..MAX_VISIBLE_COLUMNS + 1).map(|i| format!("f{i}")).collect();
    let err = set(&mut conn, workspace_id, "task", &over).await.unwrap_err();

    assert!(
        format!("{err:?}").contains("at most"),
        "expected a column-count refusal, got {err:?}"
    );
    assert_eq!(
        get(&mut conn, workspace_id, "task").await.unwrap().unwrap().columns.len(),
        MAX_VISIBLE_COLUMNS,
        "the refused save must not have replaced the accepted one"
    );
}

/// A field the schema no longer defines stays stored rather than being cleaned up on write.
/// Cleaning it here would mean a schema migration has to know about display settings; the renderer skips what it cannot find instead.
#[sqlx::test(migrations = "../../../migrations")]
async fn a_column_the_schema_no_longer_defines_is_kept(pool: PgPool) {
    let (_tenant_id, workspace_id) = seed_tenant_and_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    set(
        &mut conn,
        workspace_id,
        "task",
        &["title".to_string(), "removed_field".to_string()],
    )
    .await
    .unwrap();

    let stored = get(&mut conn, workspace_id, "task").await.unwrap().unwrap();
    assert!(stored.columns.contains(&"removed_field".to_string()));
}
