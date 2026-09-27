use crate::requests::boot_request;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use yorishiro::app::App;
use yorishiro::models::_entities::{template_templates, tenant_tenants};
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
            tags: vec![],
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
        tags: vec![],
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
async fn list_and_get_respect_tenant_and_community_visibility() {
    if !super::super::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let tenant_a = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-a".into()),
            ..Default::default()
        };
        let tenant_a = sea_orm::ActiveModelTrait::insert(tenant_a, &ctx.db)
            .await
            .expect("insert tenant a");

        let tenant_b = tenant_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-b".into()),
            ..Default::default()
        };
        let tenant_b = sea_orm::ActiveModelTrait::insert(tenant_b, &ctx.db)
            .await
            .expect("insert tenant b");

        // Tenant A's own private template.
        let private = template_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant_a.id),
            name: sea_orm::ActiveValue::Set("a-private".into()),
            definition: sea_orm::ActiveValue::Set(note_definition("a-private")),
            visibility: sea_orm::ActiveValue::Set("tenant".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let private = sea_orm::ActiveModelTrait::insert(private, &ctx.db)
            .await
            .expect("insert private template");

        // Tenant B's community-visible template.
        let community = template_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant_b.id),
            name: sea_orm::ActiveValue::Set("b-community".into()),
            definition: sea_orm::ActiveValue::Set(note_definition("b-community")),
            visibility: sea_orm::ActiveValue::Set("community".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let community = sea_orm::ActiveModelTrait::insert(community, &ctx.db)
            .await
            .expect("insert community template");

        // Tenant B's own private template, which tenant A must never see.
        let hidden = template_templates::ActiveModel {
            tenant_id: sea_orm::ActiveValue::Set(tenant_b.id),
            name: sea_orm::ActiveValue::Set("b-private".into()),
            definition: sea_orm::ActiveValue::Set(note_definition("b-private")),
            visibility: sea_orm::ActiveValue::Set("tenant".into()),
            tags: sea_orm::ActiveValue::Set(vec![]),
            ..Default::default()
        };
        let hidden = sea_orm::ActiveModelTrait::insert(hidden, &ctx.db)
            .await
            .expect("insert hidden template");

        let visible = templates::list_templates(
            &ctx.db,
            tenant_a.id,
            yorishiro::models::pagination::ListParams::default(),
        )
        .await
        .expect("list_templates");
        let names: Vec<&str> = visible.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"a-private"),
            "own template must be visible: {names:?}"
        );
        assert!(
            names.contains(&"b-community"),
            "community template must be visible: {names:?}"
        );
        assert!(
            !names.contains(&"b-private"),
            "another tenant's private template must not be visible: {names:?}"
        );

        templates::get_template(&ctx.db, tenant_a.id, private.id)
            .await
            .expect("own template is gettable");
        templates::get_template(&ctx.db, tenant_a.id, community.id)
            .await
            .expect("community template is gettable");
        let denied = templates::get_template(&ctx.db, tenant_a.id, hidden.id).await;
        assert!(
            denied.is_err(),
            "another tenant's private template must 404, got {denied:?}"
        );
    })
    .await;
}
