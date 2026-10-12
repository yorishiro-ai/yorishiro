//! `seed_official_templates` has no HTTP surface (it is a Loco task, `cargo loco task seed_official_templates`), so this calls the service function directly against `ctx.db`, matching how `tests/requests/stripe.rs` calls `tenant_billing::` functions directly alongside HTTP requests in the same suite.

use super::boot_request;
use loco_rs::app::Hooks;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use yorishiro::App;
use yorishiro::edition::ee::models::template_templates::{
    self as official_templates, OFFICIAL_TENANT_ID,
};
use yorishiro::models::_entities::{template_versions, tenant_tenants};

/// A first run publishes every built-in template and creates the official tenant; a second run republishes nothing.
#[tokio::test]
async fn seeding_is_idempotent_and_creates_the_official_tenant() {
    // Official templates are published as marketplace versions, which are PostgreSQL-only.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|_request, ctx| async move {
        let built_in_count = yorishiro::data::templates::list_templates().len();

        let first = official_templates::seed_official_templates(&ctx)
            .await
            .expect("first seed run");
        assert_eq!(first.published.len(), built_in_count);
        assert_eq!(first.updated.len(), 0);
        assert_eq!(first.unchanged.len(), 0);

        let tenant = tenant_tenants::Entity::find()
            .filter(tenant_tenants::Column::Id.eq(OFFICIAL_TENANT_ID))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(
            tenant.is_some(),
            "the official tenant row must exist after seeding"
        );
        let published_statuses = template_versions::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .map(|version| version.status)
            .collect::<Vec<_>>();
        assert!(!published_statuses.is_empty());
        assert!(published_statuses.iter().all(|status| status == "stable"));

        let second = official_templates::seed_official_templates(&ctx)
            .await
            .expect("second seed run");
        assert_eq!(
            second.published.len(),
            0,
            "an unchanged built-in must not be published again"
        );
        assert_eq!(second.updated.len(), 0);
        assert_eq!(second.unchanged.len(), built_in_count);

        let task_template = yorishiro::models::_entities::template_templates::Entity::find()
            .filter(yorishiro::models::_entities::template_templates::Column::Name.eq("task-management"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let world_template = yorishiro::models::_entities::template_templates::Entity::find()
            .filter(yorishiro::models::_entities::template_templates::Column::Name.eq("worldbuilding"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        for template in [task_template, world_template] {
            let latest = template_versions::Entity::find()
                .filter(template_versions::Column::TemplateId.eq(template.id))
                .order_by_desc(template_versions::Column::Version)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let mut previous = latest.definition;
            if template.name == "task-management" {
                for entity in ["client", "milestone", "assignee", "work_session", "handoff"] {
                    previous["entity_types"].as_object_mut().unwrap().remove(entity);
                }
                for relation in [
                    "for_client", "milestone_of", "targets_milestone", "assigned_to", "worked_on",
                    "performed_by", "handoff_from", "handoff_to", "handoff_about",
                ] {
                    previous["relation_types"].as_object_mut().unwrap().remove(relation);
                }
            } else {
                previous["description"] = serde_json::json!(
                    "Characters, locations, factions, and items for creative writing and TRPG worldbuilding"
                );
            }
            let active = template_versions::ActiveModel {
                id: ActiveValue::Set(latest.id),
                definition: ActiveValue::Set(previous),
                ..Default::default()
            };
            active.update(&ctx.db).await.unwrap();
        }
        let changed = official_templates::seed_official_templates(&ctx)
            .await
            .expect("seed changed definitions");
        assert_eq!(changed.published.len(), 0);
        assert_eq!(changed.updated.len(), 2);
        assert_eq!(changed.unchanged.len(), built_in_count - 2);
    })
    .await;
}

/// The official tenant must exist after `Hooks::seed` even without running `seed_official_templates`.
#[tokio::test]
async fn hooks_seed_creates_the_official_tenant_without_publishing_templates() {
    boot_request::<App, _, _>(|_request, ctx| async move {
        App::seed(&ctx, std::path::Path::new("does-not-need-to-exist"))
            .await
            .expect("Hooks::seed");

        let tenant = tenant_tenants::Entity::find()
            .filter(tenant_tenants::Column::Id.eq(OFFICIAL_TENANT_ID))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(
            tenant.is_some(),
            "Hooks::seed must create the official tenant on its own"
        );
    })
    .await;
}
