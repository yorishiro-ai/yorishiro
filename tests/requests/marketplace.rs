use super::boot_request;
use chrono::Utc;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::ee::services::licence::{LicenceClaims, LicenceState};
use yorishiro::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro::models::tenancy::{self, MembershipRole};
use yorishiro::services::auth::ApiKeyScope;

/// `shared_store.insert` is keyed by `TypeId` (see `App::after_context`'s own doc comment), so this overwrites the `LicenceState::from_env()` the test process booted with, the same way production code layers a later insert over an earlier one.
/// Simpler than round-tripping a real RSA-signed token through `YORISHIRO_LICENSE_KEY`, which `services::licence`'s own tests already cover.
fn licence(ctx: &loco_rs::app::AppContext) {
    ctx.shared_store
        .insert(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        }));
}

struct Setup {
    tenant_id: Uuid,
    owner_key: String,
}

/// Builds a tenant, its one workspace, and an owner with a migration-scope key, directly on `ctx.db`: same shape as base's own `tests/requests/template_library.rs`, and needed here because `POST /setup` refuses a second call once any tenant exists (`setup_bootstraps_once_and_refuses_a_second_call`), which every test below that needs two tenants would otherwise hit.
async fn setup(ctx: &loco_rs::app::AppContext, name: &str) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set(name.to_string()),
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

    let owner = tenancy::create_user(
        &ctx.db,
        &format!("owner-{name}@example.com"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let owner_key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Migration,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue owner key")
    .plaintext;

    Setup {
        tenant_id: tenant.id,
        owner_key,
    }
}

fn note_definition() -> serde_json::Value {
    json!({
        "name": "crm-starter",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

/// An unlicensed deployment answers 404, matching an unconfigured setup wizard: the deployment genuinely does not serve this, not 401/403, which would confirm the route exists.
#[tokio::test]
#[serial]
async fn without_a_licence_the_marketplace_is_not_served() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let setup = setup(&ctx, "acme").await;

        let response = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", setup.owner_key))
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// The gate runs before authentication: an unlicensed deployment answers the same 404 whether or not the caller holds a valid key, so an anonymous prober cannot tell the route exists.
#[tokio::test]
#[serial]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, _ctx| async move {
        let response = request
            .get("/api/marketplace")
            .add_header("Authorization", "Bearer ysr_not_a_real_key")
            .await;
        assert_eq!(
            response.status_code(),
            404,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// A licensed deployment still authenticates: without this, "gated" and "open to anyone" would look the same as the previous test.
#[tokio::test]
#[serial]
async fn a_licence_does_not_replace_authentication() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);

        let response = request
            .get("/api/marketplace")
            .add_header("Authorization", "Bearer ysr_not_a_real_key")
            .await;
        assert_eq!(
            response.status_code(),
            401,
            "response: {:?}",
            response.text()
        );
    })
    .await;
}

/// The full publish -> list -> fork -> review round trip, and the decisions along the way: a draft never appears in the public listing, version numbers are assigned server-side, and a fork is a private ('tenant') copy of a forker's own, distinct from the original.
#[tokio::test]
#[serial]
async fn publish_list_fork_and_review_round_trip() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let owner = setup(&ctx, "acme").await;

        // Publishing a version of a template that doesn't exist must 404.
        let draft = request
            .post("/api/marketplace/00000000-0000-0000-0000-000000000000/versions")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"definition": note_definition()}))
            .await;
        assert_eq!(draft.status_code(), 404, "response: {:?}", draft.text());

        // Create a template via base's own template library, then publish it visible.
        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({
                "name": "crm-starter",
                "definition": note_definition(),
            }))
            .await;
        assert_eq!(
            create_template.status_code(),
            201,
            "response: {:?}",
            create_template.text()
        );
        let template_id: Uuid = create_template.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let make_community = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"visibility": "community"}))
            .await;
        assert_eq!(
            make_community.status_code(),
            204,
            "response: {:?}",
            make_community.text()
        );

        // Still not listed: no published (non-draft) version exists yet.
        let empty_listing = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .await;
        assert_eq!(empty_listing.status_code(), 200);
        let empty_body: Vec<serde_json::Value> = empty_listing.json();
        assert!(
            empty_body.is_empty(),
            "a community template with no published version must not be listed: {empty_body:?}"
        );

        let publish_v1 = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;
        assert_eq!(
            publish_v1.status_code(),
            201,
            "response: {:?}",
            publish_v1.text()
        );
        assert_eq!(
            publish_v1.json::<serde_json::Value>()["version"].as_i64(),
            Some(1),
            "the first published version must be numbered 1"
        );

        let publish_v2 = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;
        assert_eq!(
            publish_v2.json::<serde_json::Value>()["version"].as_i64(),
            Some(2),
            "the next publish must be numbered one past the last, not caller-supplied"
        );

        let listing = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .await;
        let listing_body: Vec<serde_json::Value> = listing.json();
        assert_eq!(listing_body.len(), 1, "listing: {listing_body:?}");
        assert_eq!(listing_body[0]["latest_stable_version"].as_i64(), Some(2));

        // A second tenant forks it.
        let forker = setup(&ctx, "beta").await;
        assert_ne!(forker.tenant_id, owner.tenant_id);

        let fork = request
            .post(&format!("/api/marketplace/{template_id}/fork"))
            .add_header("Authorization", format!("Bearer {}", forker.owner_key))
            .await;
        assert_eq!(fork.status_code(), 201, "response: {:?}", fork.text());
        let forked_id: Uuid = fork.json::<serde_json::Value>()["template_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(
            forked_id, template_id,
            "a fork is a new template, not the original"
        );

        // The fork does not itself appear in the marketplace: it starts private ('tenant').
        let listing_after_fork = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .await;
        let listing_after_fork_body: Vec<serde_json::Value> = listing_after_fork.json();
        assert_eq!(
            listing_after_fork_body.len(),
            1,
            "a freshly forked template must not itself be community-visible: \
             {listing_after_fork_body:?}"
        );

        // The forker reviews the original.
        let review = request
            .post(&format!("/api/marketplace/{template_id}/reviews"))
            .add_header("Authorization", format!("Bearer {}", forker.owner_key))
            .json(&json!({"rating": 5, "comment": "does the job"}))
            .await;
        assert_eq!(review.status_code(), 200, "response: {:?}", review.text());

        let listing_with_review = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .await;
        let listing_with_review_body: Vec<serde_json::Value> = listing_with_review.json();
        assert_eq!(
            listing_with_review_body[0]["review_count"].as_i64(),
            Some(1)
        );
        assert_eq!(
            listing_with_review_body[0]["average_rating"].as_f64(),
            Some(5.0)
        );

        // Forking a template that has been taken back down to 'tenant' visibility 404s.
        let take_down = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"visibility": "tenant"}))
            .await;
        assert_eq!(take_down.status_code(), 204);

        let fork_after_takedown = request
            .post(&format!("/api/marketplace/{template_id}/fork"))
            .add_header("Authorization", format!("Bearer {}", forker.owner_key))
            .await;
        assert_eq!(
            fork_after_takedown.status_code(),
            404,
            "a template no longer community-visible must not be forkable by another tenant: \
             {:?}",
            fork_after_takedown.text()
        );
    })
    .await;
}

/// A tenant may not set another tenant's template's visibility, or publish a version onto it: ownership is enforced by the service, not the role, and is reported as 404 rather than 403 so a caller that cannot act on a template does not learn it exists from the difference.
#[tokio::test]
#[serial]
async fn another_tenant_cannot_manage_a_template_it_does_not_own() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let owner = setup(&ctx, "acme").await;
        let other = setup(&ctx, "beta").await;

        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"name": "private-one", "definition": note_definition()}))
            .await;
        let template_id: Uuid = create_template.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let steal_visibility = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", other.owner_key))
            .json(&json!({"visibility": "community"}))
            .await;
        assert_eq!(
            steal_visibility.status_code(),
            404,
            "response: {:?}",
            steal_visibility.text()
        );

        let steal_publish = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", other.owner_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;
        assert_eq!(
            steal_publish.status_code(),
            404,
            "response: {:?}",
            steal_publish.text()
        );
    })
    .await;
}

/// A rating outside 1-5 is rejected before it ever reaches the database.
#[tokio::test]
#[serial]
async fn a_rating_outside_the_range_is_rejected() {
    if super::super::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        licence(&ctx);
        let owner = setup(&ctx, "acme").await;
        let reviewer = setup(&ctx, "beta").await;

        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"name": "reviewable", "definition": note_definition()}))
            .await;
        let template_id: Uuid = create_template.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"visibility": "community"}))
            .await;
        request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.owner_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;

        let bad_review = request
            .post(&format!("/api/marketplace/{template_id}/reviews"))
            .add_header("Authorization", format!("Bearer {}", reviewer.owner_key))
            .json(&json!({"rating": 6}))
            .await;
        assert_eq!(
            bad_review.status_code(),
            422,
            "response: {:?}",
            bad_review.text()
        );
    })
    .await;
}
