use super::boot_request;
use axum::http::StatusCode;
use yorishiro::App;
use yorishiro::data::templates::TEMPLATES;
use yorishiro::models::api_keys::ApiKeyScope;

use super::fixtures::{TenantArgs, TestTenant};

mod deletion;

/// Creates the next schema version from the definition and returns its id.
async fn create_schema_version(
    request: &loco_rs::TestServer,
    key: &str,
    definition: &serde_json::Value,
) -> String {
    let response = request
        .post("/api/schemas")
        .add_header("Authorization", format!("Bearer {key}"))
        .json(definition)
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<serde_json::Value>()["schema"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn schema_deletion_refuses_active_versions_and_references() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            api_key: key,
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let definition = serde_json::json!({
            "name": "delete-history",
            "entity_types": {
                "note": {
                    "fields": { "title": { "type": "string" } }
                }
            }
        });
        // The workspace binds to the first version it activates and keeps that binding.
        let v1_id = create_schema_version(&request, &key, &definition).await;
        let v2_id = create_schema_version(&request, &key, &definition).await;
        let v3_id = create_schema_version(&request, &key, &definition).await;

        request
            .delete(&format!("/api/schemas/{v2_id}"))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
        let read_key =
            super::fixtures::issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false)
                .await;
        request
            .delete(&format!("/api/schemas/{v2_id}"))
            .add_header("Authorization", format!("Bearer {read_key}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);

        // An active version cannot be deleted.
        request
            .delete(&format!("/api/schemas/{v3_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::CONFLICT);

        // v1 is archived but the workspace still binds it, so it is refused with the blocker named.
        let bound = request
            .delete(&format!("/api/schemas/{v1_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        bound.assert_status(StatusCode::CONFLICT);
        assert!(
            bound.text().contains("workspaces"),
            "response: {:?}",
            bound.text()
        );

        // v2 is archived and unbound, so it is deleted.
        let barrier = tokio::sync::Barrier::new(2);
        let delete = async || {
            barrier.wait().await;
            request
                .delete(&format!("/api/schemas/{v2_id}"))
                .add_header("Authorization", format!("Bearer {key}"))
                .await
        };
        let (first, second) = tokio::join!(delete(), delete());
        let mut statuses = [first.status_code().as_u16(), second.status_code().as_u16()];
        statuses.sort();
        assert_eq!(
            statuses,
            [204, 404],
            "first: {}, second: {}",
            first.text(),
            second.text()
        );
        request
            .get(&format!("/api/schemas/{v2_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);

        // Entities referencing a version block its deletion after it is archived.
        let entity = request
            .post("/api/entities")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({
                "schema_name": "delete-history",
                "entity_type": "note",
                "data": { "title": "kept" }
            }))
            .await;
        entity.assert_status(StatusCode::CREATED);
        let entity_id = entity.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        create_schema_version(&request, &key, &definition).await;
        let blocked = request
            .delete(&format!("/api/schemas/{v3_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        blocked.assert_status(StatusCode::CONFLICT);
        assert!(
            blocked.text().contains("entities"),
            "response: {:?}",
            blocked.text()
        );
        request
            .get(&format!("/api/entities/{entity_id}"))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status_ok();

        // An unknown id is a 404, not a conflict.
        request
            .delete(&format!("/api/schemas/{}", uuid::Uuid::new_v4()))
            .add_header("Authorization", format!("Bearer {key}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn create_schema_from_a_builtin_template() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "task-management" }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["name"], "task-management");
        assert!(
            body["schema"]["definition"]["entity_types"]["task"].is_object(),
            "body: {body}"
        );
    })
    .await;
}

#[tokio::test]
async fn create_schema_from_every_builtin_template() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        for template in TEMPLATES {
            let response = request
                .post("/api/schemas")
                .add_header("Authorization", format!("Bearer {key}"))
                .json(&serde_json::json!({ "template_id": template.id }))
                .await;
            assert_eq!(
                response.status_code(),
                StatusCode::CREATED,
                "{}: {}",
                template.id,
                response.text()
            );
        }
    })
    .await;
}

#[tokio::test]
async fn create_schema_rejects_an_unknown_template_id() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": "no-such-template" }))
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

#[tokio::test]
async fn list_templates_and_get_template_over_rest() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let response = request
            .get("/api/templates")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        let ids: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "general-notes",
                "task-management",
                "creative-writing",
                "trpg-campaign",
                "game-dev-lore",
                "research-knowledge-base",
                "worldbuilding",
                "software-adr"
            ]
        );

        let response = request
            .get("/api/templates/task-management")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        let body: serde_json::Value = response.json();
        assert_eq!(body["name"], "task-management");

        let response = request
            .get("/api/templates/worldbuilding")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(response.status_code(), StatusCode::OK);
        assert!(
            response.json::<serde_json::Value>()["description"]
                .as_str()
                .unwrap()
                .contains("Deprecated")
        );
    })
    .await;
}

/// A UUID `template_id` names a row in the tenant's own library rather than a built-in template, and resolving it stamps `origin_template_id`/`origin_status`/`origin_snapshot` onto the created schema.
#[tokio::test]
async fn create_schema_from_a_library_template_links_the_origin() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let name = format!("library-note-{}", uuid::Uuid::new_v4().simple());
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({
                "name": name,
                "definition": {
                    "name": name,
                    "entity_types": {
                        "note": { "fields": { "title": { "type": "string", "required": true } } }
                    }
                }
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let template_id = response.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("created template ID")
            .to_owned();

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": template_id }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["name"], name);
        assert_eq!(body["schema"]["origin_template_id"], template_id);
        assert_eq!(body["schema"]["origin_status"], "linked");
        assert!(
            body["schema"]["origin_snapshot"].is_object(),
            "body: {body}"
        );
    })
    .await;
}

/// A caller passing no origin on a second version must not silently un-link a schema that was created from a template: `schema_schemas::create_schema` inherits the previous active version's origin when the caller passes `None`.
#[tokio::test]
async fn a_second_version_with_no_origin_inherits_the_first_versions_link() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let name = format!("library-note-{}", uuid::Uuid::new_v4().simple());
        let TestTenant { api_key: key, .. } = TestTenant::create(
            &request,
            &ctx,
            TenantArgs {
                key_scope: ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({
                "name": name,
                "definition": {
                    "name": name,
                    "entity_types": {
                        "note": { "fields": { "title": { "type": "string", "required": true } } }
                    }
                }
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::CREATED);
        let template_id = response.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("created template ID")
            .to_owned();

        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({ "template_id": template_id }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );

        // A second version, posted as an inline definition (no template_id at all): the REST body shape has no way to say "same origin as before", so this is exactly the case an operator hits editing a linked schema by hand.
        let response = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({
                "name": name,
                "entity_types": {
                    "note": {
                        "fields": {
                            "title": { "type": "string", "required": true },
                            "body": { "type": "string", "required": false }
                        }
                    }
                }
            }))
            .await;
        assert_eq!(
            response.status_code(),
            201,
            "response: {:?}",
            response.text()
        );
        let body: serde_json::Value = response.json();
        assert_eq!(body["schema"]["version"], 2);
        assert_eq!(
            body["schema"]["origin_template_id"], template_id,
            "the link to the library template must survive an edit with no explicit origin: {body}"
        );
        assert_eq!(body["schema"]["origin_status"], "linked");
    })
    .await;
}
