use loco_rs::testing::prelude::*;
use serial_test::serial;
use yorishiro::app::App;
use yorishiro::models::_entities::{identity_templates, identity_tenants};
use yorishiro::models::identity_templates as templates;

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

#[tokio::test]
#[serial]
async fn list_and_get_respect_tenant_and_community_visibility() {
    request_with_create_db::<App, _, _>(|_request, ctx| async move {
        let tenant_a = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-a".into()),
            ..Default::default()
        };
        let tenant_a = sea_orm::ActiveModelTrait::insert(tenant_a, &ctx.db)
            .await
            .expect("insert tenant a");

        let tenant_b = identity_tenants::ActiveModel {
            name: sea_orm::ActiveValue::Set("tenant-b".into()),
            ..Default::default()
        };
        let tenant_b = sea_orm::ActiveModelTrait::insert(tenant_b, &ctx.db)
            .await
            .expect("insert tenant b");

        // Tenant A's own private template.
        let private = identity_templates::ActiveModel {
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
        let community = identity_templates::ActiveModel {
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
        let hidden = identity_templates::ActiveModel {
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
