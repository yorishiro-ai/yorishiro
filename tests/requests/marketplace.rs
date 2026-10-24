use super::boot_request;
use axum::http::StatusCode;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;

use super::fixtures::{TenantArgs, TestTenant};

fn require_licence(ctx: &loco_rs::app::AppContext) {
    assert!(
        ctx.shared_store
            .get::<std::sync::Arc<LicenceState>>()
            .expect("production boot installs licence state")
            .is_active(),
        "licensed marketplace tests require a valid YORISHIRO_LICENSE_KEY at boot"
    );
}

fn note_definition() -> serde_json::Value {
    json!({
        "name": "crm-starter",
        "entity_types": {
            "note": { "fields": { "title": { "type": "string", "required": true } } }
        }
    })
}

async fn listings(request: &loco_rs::TestServer, key: &str) -> Vec<serde_json::Value> {
    let mut templates = Vec::new();
    for page in 1.. {
        let response = request
            .get(&format!("/api/marketplace?page={page}&page_size=200"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let batch: Vec<serde_json::Value> = response.json();
        let last = batch.len() < 200;
        templates.extend(batch);
        if last {
            return templates;
        }
    }
    unreachable!()
}

/// An unlicensed deployment answers 404, matching an unconfigured setup wizard: the deployment genuinely does not serve this, not 401/403, which would confirm the route exists.
#[tokio::test]
#[serial(process_environment)]
async fn without_a_licence_the_marketplace_is_not_served() {
    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY"]);
    guard.remove("YORISHIRO_LICENSE_KEY");
    super::boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let setup = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let response = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", setup.api_key))
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
#[serial(process_environment)]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    let guard = crate::EnvGuard::capture(&["YORISHIRO_LICENSE_KEY"]);
    guard.remove("YORISHIRO_LICENSE_KEY");
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
async fn a_licence_does_not_replace_authentication() {
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);

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

/// SQLite holds one tenant, so the cross-tenant marketplace answers `501` with a stable code rather than failing inside a query.
#[tokio::test]
async fn the_marketplace_answers_501_on_sqlite() {
    if !crate::require_sqlite_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let owner = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let listing = request
            .get("/api/marketplace")
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .await;
        assert_eq!(listing.status_code(), StatusCode::NOT_IMPLEMENTED);
        let body: serde_json::Value = listing.json();
        assert_eq!(body["error"]["code"], "backend_unsupported");
    })
    .await;
}

/// The full publish -> list -> fork -> review round trip, and the decisions along the way: a draft never appears in the public listing, version numbers are assigned server-side, and a fork is a private ('tenant') copy of a forker's own, distinct from the original.
#[tokio::test]
async fn publish_list_fork_and_review_round_trip() {
    // The marketplace spans tenants, so its listing and publishing queries are PostgreSQL-only.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let owner = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        // Publishing a version of a template that doesn't exist must 404.
        let draft = request
            .post("/api/marketplace/00000000-0000-0000-0000-000000000000/versions")
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition()}))
            .await;
        assert_eq!(
            draft.status_code(),
            StatusCode::NOT_FOUND,
            "response: {:?}",
            draft.text()
        );

        // Create a template via base's own template library, then publish it visible.
        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
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

        let invalid_visibility = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"visibility": "paused"}))
            .await;
        assert_eq!(
            invalid_visibility.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let invalid_body: serde_json::Value = invalid_visibility.json();
        assert_eq!(
            invalid_body["error"]["message"],
            "unknown visibility 'paused'"
        );
        assert_eq!(
            invalid_body["error"]["hint"],
            "use 'tenant' to keep it private or 'community' to list it"
        );

        let unchanged = request
            .get(&format!("/api/template-library/{template_id}"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .await;
        assert_eq!(unchanged.status_code(), StatusCode::OK);
        assert_eq!(
            unchanged.json::<serde_json::Value>()["visibility"],
            "tenant"
        );

        let make_community = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"visibility": "community"}))
            .await;
        assert_eq!(
            make_community.status_code(),
            204,
            "response: {:?}",
            make_community.text()
        );

        // Still not listed: no published (non-draft) version exists yet.
        let empty_body = listings(&request, &owner.api_key).await;
        assert!(
            empty_body
                .iter()
                .all(|template| template["template_id"] != template_id.to_string()),
            "a community template with no published version must not be listed: {empty_body:?}"
        );

        let invalid_publish = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition(), "status": "paused"}))
            .await;
        assert_eq!(
            invalid_publish.status_code(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let versions_after_invalid: Vec<serde_json::Value> = request
            .get(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .await
            .json();
        assert!(versions_after_invalid.is_empty());

        let publish_draft = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition()}))
            .await;
        assert_eq!(
            publish_draft.status_code(),
            201,
            "response: {:?}",
            publish_draft.text()
        );
        assert_eq!(publish_draft.json::<serde_json::Value>()["status"], "draft");

        let publish_v1 = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;
        assert_eq!(
            publish_v1.json::<serde_json::Value>()["version"].as_i64(),
            Some(2),
            "the second version must be numbered 2"
        );

        let publish_pre = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition(), "status": "pre"}))
            .await;
        assert_eq!(publish_pre.status_code(), StatusCode::CREATED);
        assert_eq!(publish_pre.json::<serde_json::Value>()["status"], "pre");

        let publish_v2 = request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;
        assert_eq!(
            publish_v2.json::<serde_json::Value>()["version"].as_i64(),
            Some(4),
            "the next publish must be numbered one past the last, not caller-supplied"
        );

        let listing_body = listings(&request, &owner.api_key).await;
        let listed = listing_body
            .iter()
            .find(|template| template["template_id"] == template_id.to_string())
            .expect("published template must be listed");
        assert_eq!(listed["latest_stable_version"].as_i64(), Some(4));

        let versions: Vec<serde_json::Value> = request
            .get(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .await
            .json();
        assert_eq!(
            versions
                .iter()
                .map(|version| version["status"].as_str())
                .collect::<Vec<_>>(),
            vec![Some("stable"), Some("pre"), Some("stable"), Some("draft")]
        );

        // A second tenant forks it.
        let forker = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        assert_ne!(forker.tenant_id, owner.tenant_id);

        let fork = request
            .post(&format!("/api/marketplace/{template_id}/fork"))
            .add_header("Authorization", format!("Bearer {}", forker.api_key))
            .await;
        assert_eq!(
            fork.status_code(),
            StatusCode::CREATED,
            "response: {:?}",
            fork.text()
        );
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
        let listing_after_fork_body = listings(&request, &owner.api_key).await;
        assert!(
            listing_after_fork_body
                .iter()
                .all(|template| template["template_id"] != forked_id.to_string()),
            "a freshly forked template must not itself be community-visible: \
             {listing_after_fork_body:?}"
        );

        // The forker reviews the original.
        let review = request
            .post(&format!("/api/marketplace/{template_id}/reviews"))
            .add_header("Authorization", format!("Bearer {}", forker.api_key))
            .json(&json!({"rating": 5, "comment": "does the job"}))
            .await;
        assert_eq!(
            review.status_code(),
            StatusCode::OK,
            "response: {:?}",
            review.text()
        );

        let listing_with_review_body = listings(&request, &owner.api_key).await;
        let reviewed = listing_with_review_body
            .iter()
            .find(|template| template["template_id"] == template_id.to_string())
            .expect("reviewed template must be listed");
        assert_eq!(reviewed["review_count"].as_i64(), Some(1));
        assert_eq!(reviewed["average_rating"].as_f64(), Some(5.0));

        // Forking a template that has been taken back down to 'tenant' visibility 404s.
        let take_down = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"visibility": "tenant"}))
            .await;
        assert_eq!(take_down.status_code(), StatusCode::NO_CONTENT);

        let fork_after_takedown = request
            .post(&format!("/api/marketplace/{template_id}/fork"))
            .add_header("Authorization", format!("Bearer {}", forker.api_key))
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
async fn another_tenant_cannot_manage_a_template_it_does_not_own() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let owner = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let other = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"name": "private-one", "definition": note_definition()}))
            .await;
        let template_id: Uuid = create_template.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let steal_visibility = request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", other.api_key))
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
            .add_header("Authorization", format!("Bearer {}", other.api_key))
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
async fn a_rating_outside_the_range_is_rejected() {
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        require_licence(&ctx);
        let owner = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let reviewer = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let create_template = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"name": "reviewable", "definition": note_definition()}))
            .await;
        let template_id: Uuid = create_template.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        request
            .put(&format!("/api/marketplace/{template_id}/visibility"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"visibility": "community"}))
            .await;
        request
            .post(&format!("/api/marketplace/{template_id}/versions"))
            .add_header("Authorization", format!("Bearer {}", owner.api_key))
            .json(&json!({"definition": note_definition(), "status": "stable"}))
            .await;

        let bad_review = request
            .post(&format!("/api/marketplace/{template_id}/reviews"))
            .add_header("Authorization", format!("Bearer {}", reviewer.api_key))
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
