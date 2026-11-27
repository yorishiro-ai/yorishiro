use chrono::Utc;
#[cfg(feature = "enterprise")]
use serde_json::json;
use uuid::Uuid;
use yorishiro::models::_entities::template_templates;
use yorishiro::models::template_templates::{self as templates, TemplateVisibility};

fn note_definition(name: &str) -> serde_json::Value {
    crate::requests::fixtures::note_schema(name, false)
}

#[test]
fn persisted_visibility_maps_to_the_record() {
    for (visibility, wire) in [
        (TemplateVisibility::Tenant, "tenant"),
        (TemplateVisibility::Community, "community"),
    ] {
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
    use crate::requests::fixtures::{TenantArgs, TestTenant};
    use yorishiro::models::pagination::ListParams;

    if !crate::require_postgres_backend() {
        return;
    }
    crate::requests::boot_multitenant_request(|request, ctx| async move {
        let owner_a = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let owner_b = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_a = owner_a.tenant_id;
        let key_a =
            crate::requests::fixtures::issue_tenant_key(&ctx, tenant_a, owner_a.owner_id).await;
        let key_b =
            crate::requests::fixtures::issue_tenant_key(&ctx, owner_b.tenant_id, owner_b.owner_id)
                .await;
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
            ids.push(Uuid::parse_str(body["template_id"].as_str().unwrap()).unwrap());
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
                "licensed template publication tests require an active licence"
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
