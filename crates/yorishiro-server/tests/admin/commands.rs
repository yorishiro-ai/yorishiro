use sqlx::PgPool;
use uuid::Uuid;

use crate::admin::commands::{create_api_key, list_api_keys, resync_embeddings, revoke_api_key};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use yorishiro_core::repositories::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::{self, ApiKeyScope};

#[derive(sea_query::Iden)]
enum Entities {
    Table,
    Id,
    Embedding,
}

async fn seed_workspace(pool: &PgPool) -> (Uuid, Uuid) {
    let tenant = tenancy::create_tenant(pool, "bootstrap-tenant", None)
        .await
        .unwrap();
    let workspace = tenancy::create_workspace(pool, tenant.id, "default", None, None)
        .await
        .unwrap();
    (tenant.id, workspace.id)
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_workspace_and_issues_a_usable_key(pool: PgPool) {
    let (_workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;

    let created = create_api_key(&pool, workspace_id, ApiKeyScope::Write, None)
        .await
        .unwrap();
    assert_eq!(created.workspace_id, workspace_id);
    assert!(created.plaintext.starts_with("ysr_"));
    assert_eq!(created.user_id, None);

    // Confirm the issued key actually authenticates, not just that creation returned Ok.
    let ctx = auth::authenticate(&pool, &created.plaintext).await.unwrap();
    assert_eq!(ctx.workspace_id, workspace_id);
    assert_eq!(ctx.scope, ApiKeyScope::Write);
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_key_creation_for_unknown_workspace(pool: PgPool) {
    let result = create_api_key(&pool, Uuid::nil(), ApiKeyScope::Read, None).await;
    let Err(err) = result else {
        panic!("key creation should fail for an unknown workspace");
    };
    assert!(err.to_string().contains("does not exist"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_for_user_is_capped_by_their_role(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "default", None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut conn, "viewer@example.com", "pw", None)
        .await
        .unwrap();
    tenancy::add_member(&mut conn, tenant.id, user.id, MembershipRole::Viewer)
        .await
        .unwrap();
    drop(conn);

    // A viewer may be issued a read-scope key...
    let created = create_api_key(&pool, workspace.id, ApiKeyScope::Read, Some(user.id))
        .await
        .unwrap();
    assert_eq!(created.user_id, Some(user.id));

    // ...but not a write- or schema-scope one.
    let result = create_api_key(&pool, workspace.id, ApiKeyScope::Write, Some(user.id)).await;
    let Err(err) = result else {
        panic!("a viewer should not be issuable a write-scope key");
    };
    assert!(err.to_string().contains("Viewer"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_rejects_a_user_who_is_not_a_member(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let workspace = tenancy::create_workspace(&pool, tenant.id, "default", None, None)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut conn, "outsider@example.com", "pw", None)
        .await
        .unwrap();
    drop(conn);

    let result = create_api_key(&pool, workspace.id, ApiKeyScope::Read, Some(user.id)).await;
    let Err(err) = result else {
        panic!("a non-member should not be issuable an api key");
    };
    assert!(err.to_string().contains("not a member"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn revoked_key_no_longer_authenticates(pool: PgPool) {
    let (_workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let created = create_api_key(&pool, workspace_id, ApiKeyScope::Read, None)
        .await
        .unwrap();
    auth::authenticate(&pool, &created.plaintext).await.unwrap();

    let listed = list_api_keys(&pool, workspace_id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.id);

    revoke_api_key(&pool, created.id).await.unwrap();

    let result = auth::authenticate(&pool, &created.plaintext).await;
    assert!(matches!(
        result,
        Err(yorishiro_core::YorishiroError::Unauthenticated)
    ));
    assert!(list_api_keys(&pool, workspace_id).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn resync_fills_missing_embeddings(pool: PgPool) {
    use async_trait::async_trait;
    use yorishiro_core::YorishiroError;
    use yorishiro_core::services::embedding::EmbeddingProvider;

    struct FixedProvider;

    #[async_trait]
    impl EmbeddingProvider for FixedProvider {
        fn dimensions(&self) -> usize {
            768
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
            Ok(texts.iter().map(|_| vec![0.2_f32; 768]).collect())
        }
    }

    let (workspace_id_tenant, workspace_id) = seed_workspace(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let definition = serde_json::from_value(serde_json::json!({
        "name": "task-management",
        "entity_types": {
            "task": {
                "fields": { "title": { "type": "string", "required": true, "x-embed": true } }
            }
        }
    }))
    .unwrap();
    yorishiro_core::repositories::schemas::create_schema(
        &mut conn,
        workspace_id_tenant,
        definition,
    )
    .await
    .unwrap();
    // core's create doesn't write the embedding (that's the adapter's background sync
    // job), so this entity reproduces one left behind by a failed sync.
    let entity = yorishiro_core::repositories::entities::create(
        &mut conn,
        workspace_id,
        yorishiro_core::repositories::entities::CreateEntityInput {
            schema_name: "task-management".into(),
            entity_type: "task".into(),
            data: serde_json::json!({ "title": "orphaned" }),
        },
        None,
    )
    .await
    .unwrap();
    drop(conn);

    let report = resync_embeddings(&pool, workspace_id, &FixedProvider)
        .await
        .unwrap();
    assert_eq!(report.candidates, 1);
    assert_eq!(report.synced, 1);
    assert_eq!(report.failed, 0);

    let (sql, values) = Query::select()
        .expr(Expr::col(Entities::Embedding).is_not_null())
        .from((Alias::new("content"), Entities::Table))
        .and_where(Expr::col(Entities::Id).eq(entity.id))
        .build_sqlx(PostgresQueryBuilder);
    let (has_embedding,): (bool,) = sqlx::query_as_with(&sql, values)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(has_embedding);
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_tenant_workspace_user_and_membership(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "acme", None).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let user = tenancy::create_user(&mut conn, "owner@example.com", "pw", None)
        .await
        .unwrap();
    tenancy::add_member(&mut conn, tenant.id, user.id, MembershipRole::Owner)
        .await
        .unwrap();
    drop(conn);

    let members = tenancy::list_members(&pool, tenant.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, user.id);
    assert_eq!(members[0].role, MembershipRole::Owner);
}

#[sqlx::test(migrations = "../../migrations")]
async fn enforces_workspace_limit_on_create_workspace(pool: PgPool) {
    let tenant = tenancy::create_tenant(&pool, "capped", Some(1))
        .await
        .unwrap();
    // create_tenant alone doesn't create a workspace here (unlike the CLI's CreateTenant
    // handler, which additionally creates a "default" one); this test drives
    // tenancy::create_workspace directly to check the cap.
    tenancy::create_workspace(&pool, tenant.id, "first", None, None)
        .await
        .unwrap();

    let err = tenancy::create_workspace(&pool, tenant.id, "second", None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        yorishiro_core::YorishiroError::Conflict { .. }
    ));
}
