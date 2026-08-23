use chrono::Utc;
use loco_rs::testing::prelude::*;
use serial_test::serial;
use uuid::Uuid;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;
use yorishiro_hosted::HostedApp;
use yorishiro_hosted::services::licence::{LicenceClaims, LicenceState};

/// `shared_store.insert` is keyed by `TypeId`, so this overwrites the `LicenceState::from_env()`
/// the test process booted with. See `marketplace.rs`'s own copy of this helper.
fn licence(ctx: &loco_rs::app::AppContext) {
    ctx.shared_store
        .insert(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        }));
}

struct Setup {
    key: String,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
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
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup { key }
}

/// Setting, reading and clearing a workspace's LLM credentials over REST. The key itself never
/// comes back from GET, only what it configured.
#[tokio::test]
#[serial]
async fn llm_key_set_get_and_clear_round_trip() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(missing.status_code(), 404, "response: {:?}", missing.text());

        let put = request
            .put("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1/",
                "model": "gpt-4o-mini",
                "api_key": "sk-secret-value"
            }))
            .await;
        assert_eq!(put.status_code(), 204, "response: {:?}", put.text());

        let get = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 200, "response: {:?}", get.text());
        let body: serde_json::Value = get.json();
        // The trailing slash is trimmed once at write time.
        assert_eq!(body["base_url"], "https://api.example.com/v1");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["configured"], true);
        let rendered = get.text();
        assert!(
            !rendered.contains("sk-secret-value"),
            "the key must never be returned: {rendered}"
        );

        let delete = request
            .delete("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(delete.status_code(), 204, "response: {:?}", delete.text());

        let after_delete = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A scheme that could never be a chat-completions endpoint is refused before anything is
/// stored, and a URL with no scheme at all is refused too rather than becoming a relative path.
#[tokio::test]
#[serial]
async fn a_non_http_base_url_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "api.example.com/v1",
        ] {
            let put = request
                .put("/hosted/workspace/llm-key")
                .add_header("Authorization", format!("Bearer {}", setup.key))
                .json(&serde_json::json!({
                    "base_url": bad_url,
                    "model": "m",
                    "api_key": "k"
                }))
                .await;
            assert_eq!(
                put.status_code(),
                422,
                "{bad_url:?} should have been refused: {:?}",
                put.text()
            );
        }

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A workspace with no credentials configured is refused with one clear error before any entity
/// is scanned, rather than reporting zero proposals in a way that reads as "nothing to infer".
#[tokio::test]
#[serial]
async fn infer_fill_without_a_configured_key_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;

        let create_schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": {
                    "note": { "fields": { "title": { "type": "string", "required": true } } }
                }
            }))
            .await;
        assert_eq!(
            create_schema.status_code(),
            201,
            "response: {:?}",
            create_schema.text()
        );

        let infer = request
            .post("/hosted/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(infer.status_code(), 422, "response: {:?}", infer.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// An unlicensed deployment answers the same 404 whether or not a valid key is presented, so an
/// anonymous prober cannot tell "does not exist" from "exists but locked". Matches
/// `marketplace`'s and `dashboard`'s own tests for the same gate.
#[tokio::test]
#[serial]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let with_key = request
            .post("/hosted/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let without_key = request.post("/hosted/schemas/active/note/infer-fill").await;

        assert_eq!(with_key.status_code(), without_key.status_code());
        assert_eq!(with_key.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Confirming a job with no proposals is refused rather than reporting nothing applied, so a
/// caller can tell "already confirmed" from "confirmed and changed nothing".
#[tokio::test]
#[serial]
async fn confirming_an_unknown_job_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let confirm = request
            .post(&format!(
                "/hosted/migration-jobs/{}/confirm",
                Uuid::new_v4()
            ))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(confirm.status_code(), 404, "response: {:?}", confirm.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}
