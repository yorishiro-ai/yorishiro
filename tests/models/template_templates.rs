#[cfg(feature = "enterprise")]
use crate::requests::boot_request;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
#[cfg(feature = "enterprise")]
use yorishiro::App;
use yorishiro::models::_entities::template_templates;
use yorishiro::models::template_templates::{self as templates, TemplateVisibility};

fn note_definition(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "entity_types": {
            "note": {
                "fields": {
                    "title": { "type": "string", "required": true }
                }
            }
        }
    })
}

#[test]
fn visibility_round_trips_db_and_json_values() {
    for (visibility, wire) in [
        (TemplateVisibility::Tenant, "tenant"),
        (TemplateVisibility::Community, "community"),
    ] {
        assert_eq!(visibility.as_db_str(), wire);
        assert_eq!(TemplateVisibility::from_db_str(wire), Some(visibility));
        assert_eq!(serde_json::to_value(visibility).unwrap(), json!(wire));
        assert_eq!(visibility.to_string(), wire);

        let record = templates::TemplateRecord::try_from(template_templates::Model {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            name: "template".into(),
            description: None,
            definition: note_definition("template"),
            locale: None,
            visibility: wire.into(),
            author: None,
            fork_of: None,
            created_by: None,
            created_at: Utc::now().fixed_offset(),
            updated_at: Utc::now().fixed_offset(),
            tags: serde_json::json!([]),
        })
        .unwrap();
        assert_eq!(record.visibility, visibility);
    }
}

#[test]
fn unknown_persisted_visibility_is_internal_and_input_is_validation_failed() {
    let row = template_templates::Model {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        name: "template".into(),
        description: None,
        definition: note_definition("template"),
        locale: None,
        visibility: "paused".into(),
        author: None,
        fork_of: None,
        created_by: None,
        created_at: Utc::now().fixed_offset(),
        updated_at: Utc::now().fixed_offset(),
        tags: serde_json::json!([]),
    };
    assert!(matches!(
        templates::TemplateRecord::try_from(row),
        Err(yorishiro::YorishiroError::Internal(_))
    ));

    let error = TemplateVisibility::parse_input("paused").unwrap_err();
    assert!(matches!(
        error,
        yorishiro::YorishiroError::ValidationFailed { .. }
    ));
}

#[tokio::test]
#[cfg(feature = "enterprise")]
async fn list_and_get_respect_tenant_and_community_visibility() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        use crate::requests::fixtures::{TenantArgs, TestTenant};
        use yorishiro::models::pagination::ListParams;

        let TestTenant {
            tenant_id: tenant_a,
            api_key: key_a,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key_b = TestTenant::create(&request, &ctx, TenantArgs::default())
            .await
            .api_key;
        let mut ids = Vec::new();
        for (key, name) in [
            (&key_a, "a-private"),
            (&key_b, "b-community"),
            (&key_b, "b-private"),
        ] {
            let response = request
                .post("/api/template-library")
                .add_header("Authorization", format!("Bearer {key}"))
                .json(&json!({"name": name, "definition": note_definition(name)}))
                .await;
            assert_eq!(response.status_code(), 201, "{}", response.text());
            let body: serde_json::Value = response.json();
            ids.push(Uuid::parse_str(body["id"].as_str().unwrap()).unwrap());
        }
        let [private, community, hidden]: [Uuid; 3] = ids.try_into().unwrap();

        {
            use std::sync::Arc;
            use yorishiro::edition::ee::controllers::middleware::edition::LicenceState;
            assert!(
                ctx.shared_store
                    .get::<Arc<LicenceState>>()
                    .expect("production boot installs licence state")
                    .is_active(),
                "licensed template publication tests require a valid YORISHIRO_LICENSE_KEY at boot"
            );
            let response = request
                .put(&format!("/api/marketplace/{community}/visibility"))
                .add_header("Authorization", format!("Bearer {key_b}"))
                .json(&json!({"visibility": "community"}))
                .await;
            assert_eq!(response.status_code(), 204, "{}", response.text());
        }

        let mut visible = Vec::new();
        let mut offset = 0;
        loop {
            let page = templates::list_templates(
                &ctx.db,
                tenant_a,
                ListParams::new(Some(200), Some(offset)),
            )
            .await
            .expect("list_templates");
            let last = page.len() < 200;
            visible.extend(page.into_iter().map(|template| template.id));
            if last {
                break;
            }
            offset += 200;
        }
        assert!(visible.contains(&private), "own template must be visible");
        assert!(
            visible.contains(&community),
            "community template must be visible"
        );
        assert!(
            !visible.contains(&hidden),
            "another tenant's private template must not be visible"
        );

        templates::get_template(&ctx.db, tenant_a, private)
            .await
            .expect("own template is gettable");
        templates::get_template(&ctx.db, tenant_a, community)
            .await
            .expect("community template is gettable");
        let denied = templates::get_template(&ctx.db, tenant_a, hidden).await;
        assert!(
            denied.is_err(),
            "another tenant's private template must 404, got {denied:?}"
        );
    })
    .await;
}
