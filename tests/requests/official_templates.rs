//! `seed_official_templates` has no HTTP surface (it is a Loco task, `cargo loco task seed_official_templates`), so this calls the service function directly against `ctx.db`, matching how `tests/requests/stripe.rs` calls `tenant_billing::` functions directly alongside HTTP requests in the same suite.

use super::boot_request_isolated as boot_request;
use loco_rs::app::Hooks;
use sea_orm::{ActiveModelTrait, ActiveValue};
use yorishiro::App;
use yorishiro::edition::ee::models::template_templates::{
    self as official_templates, OFFICIAL_TENANT_ID,
};
use yorishiro::models::_entities::template_versions;
use yorishiro::models::pagination::ListParams;

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

        let tenant = yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, OFFICIAL_TENANT_ID)
            .await
            .unwrap();
        assert_eq!(tenant.id, OFFICIAL_TENANT_ID);

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

        let templates = yorishiro::models::template_templates::list_templates(
            &ctx.db,
            OFFICIAL_TENANT_ID,
            ListParams::new(Some(200), None),
        )
        .await
        .unwrap();
        let templates = templates.into_iter().filter(|template| template.tenant_id == OFFICIAL_TENANT_ID).collect::<Vec<_>>();
        assert_eq!(templates.len(), built_in_count);
        for template in templates {
            let latest = yorishiro::edition::ee::models::marketplace::list_versions(
                &ctx.db,
                OFFICIAL_TENANT_ID,
                template.id,
                ListParams::new(Some(1), None),
            )
                .await
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(latest.status.as_db_str(), "stable");
            if !matches!(template.name.as_str(), "task-management" | "worldbuilding") {
                continue;
            }
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
            // Simulate a version published by an older binary; the current publisher only ships current definitions.
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

        let tenant =
            yorishiro::models::tenant_tenants::Model::find_by_id(&ctx.db, OFFICIAL_TENANT_ID)
                .await
                .unwrap();
        assert_eq!(tenant.id, OFFICIAL_TENANT_ID);
    })
    .await;
}
