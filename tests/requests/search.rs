use super::boot_request;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

struct Setup {
    read_key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("search-req-test".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let read_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Read,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue read key")
    .plaintext;
    Setup { read_key }
}

/// `YORISHIRO_EMBEDDING_PROVIDER=none` forces `build_embedding_provider` to return `UnconfiguredEmbeddingProvider` regardless of cached model files, so any actual search attempt surfaces as 502, not a panic or a silent empty result.
/// Search fails loudly and namedly rather than the boot process itself failing for every deployment that hasn't configured embeddings yet.
#[tokio::test]
#[serial]
async fn search_with_no_embedding_provider_configured_returns_502() {
    // Force unconfigured provider even if model files exist in cache: the test asserts on the
    // provider-missing path, not on the local provider succeeding.
    unsafe { std::env::set_var("YORISHIRO_EMBEDDING_PROVIDER", "none") };

    boot_request::<App, _, _>(|request, ctx| async move {
        let Setup { read_key } = setup(&ctx).await;

        let response = request
            .get("/api/search?query_text=hello")
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await;
        assert_eq!(
            response.status_code(),
            502,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// The auth check (`Verified`) runs before the embedding call: no key at all must be rejected with 401, not 502.
/// `Read` is the lowest scope, so there's no "too-low scope" case to test against a read-gated endpoint beyond this.
#[tokio::test]
#[serial]
async fn search_requires_authentication() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request.get("/api/search?query_text=hello").await;
        assert_eq!(response.status_code(), 401);
    })
    .await;
}
