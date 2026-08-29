//! Publishes the community edition's built-in templates as official marketplace listings.
//! Bypasses `services::marketplace::publish_version`'s ownership check, since the seed has no authenticated tenant to check ownership against.
//! Invoked from a Loco task (`register_tasks`), not a request path.

use crate::db;
use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{identity_template_versions, identity_tenants};
use crate::models::tenancy::INFRASTRUCTURE_TENANT_ID;
use loco_rs::app::AppContext;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use uuid::Uuid;

/// The tenant that owns the officially published templates.
/// A fixed id, so re-running the seed finds the same row rather than creating a second publisher.
pub const OFFICIAL_TENANT_ID: Uuid = INFRASTRUCTURE_TENANT_ID;

/// Shown as the listing's author.
/// `identity_templates.author` is free text, so this does not require a user account.
pub const OFFICIAL_AUTHOR: &str = "Yorishiro";

const OFFICIAL_TENANT_NAME: &str = "Yorishiro Official";

/// What one seeding run did.
/// Callers report it; nothing branches on it.
#[derive(Debug, Default)]
pub struct SeedOutcome {
    pub published: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Publishes every built-in template as an official, community-visible marketplace listing.
/// Idempotent: a new version is published only when the built-in definition differs from the latest one already published.
/// Calls `ensure_official_tenant` itself, so this still works standalone.
pub async fn seed_official_templates(ctx: &AppContext) -> Result<SeedOutcome, YorishiroError> {
    ensure_official_tenant(&ctx.db).await?;

    let mut outcome = SeedOutcome::default();

    for summary in crate::templates::list_templates() {
        let definition = crate::templates::get_template(&summary.id)?;
        let definition_json = serde_json::to_value(&definition).internal()?;

        let template_id = upsert_template(
            &ctx.db,
            &summary.id,
            summary.description.as_deref(),
            &definition_json,
        )
        .await?;

        // Compare against the newest version of any status, not just `stable`: publishing a fresh version every run would otherwise walk the version number up forever while the definition stayed the same.
        let latest = identity_template_versions::Entity::find()
            .filter(identity_template_versions::Column::TemplateId.eq(template_id))
            .order_by_desc(identity_template_versions::Column::Version)
            .limit(1)
            .one(&ctx.db)
            .await
            .internal()?;

        match latest {
            Some(latest) if latest.definition == definition_json => {
                outcome.unchanged.push(summary.id.clone());
                continue;
            }
            Some(_) => outcome.updated.push(summary.id.clone()),
            None => outcome.published.push(summary.id.clone()),
        }

        // `stable`, not `draft`: a draft is visible only to its owning tenant, and this tenant has no members to view it.
        let request = crate::ee::models::marketplace::PublishVersionRequest {
            definition: definition_json,
            changelog: Some(format!("Built-in template '{}'", summary.id)),
            status: "stable".to_string(),
        };
        // lock_for_update is transaction-scoped, so this needs its own txn rather than ctx.db.
        let txn = ctx.db.begin().await.internal()?;
        db::lock_for_update(&txn, &format!("template-version:{template_id}"))
            .await
            .internal()?;
        crate::ee::models::marketplace::insert_next_version(&txn, template_id, &request, None)
            .await?;
        txn.commit().await.internal()?;
    }

    Ok(outcome)
}

/// Creates the official-templates publisher tenant if it does not already exist.
/// Idempotent (`ON CONFLICT DO NOTHING` on the fixed id).
pub async fn ensure_official_tenant(conn: &impl ConnectionTrait) -> Result<(), YorishiroError> {
    // Bypasses tenancy::create_tenant: the publisher is infrastructure, not subject to YORISHIRO_MAX_TENANTS.
    let active = identity_tenants::ActiveModel {
        id: ActiveValue::Set(OFFICIAL_TENANT_ID),
        name: ActiveValue::Set(OFFICIAL_TENANT_NAME.to_string()),
        ..Default::default()
    };
    identity_tenants::Entity::insert(active)
        .on_conflict(
            OnConflict::column(identity_tenants::Column::Id)
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(conn)
        .await
        .internal()?;
    Ok(())
}

/// Creates the template row, or refreshes the description/definition of the existing one.
/// `identity_templates` is unique on `(tenant_id, name)`, which is what makes this idempotent.
async fn upsert_template(
    conn: &impl ConnectionTrait,
    name: &str,
    description: Option<&str>,
    definition: &serde_json::Value,
) -> Result<Uuid, YorishiroError> {
    use crate::models::_entities::identity_templates::{ActiveModel, Column, Entity};

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(OFFICIAL_TENANT_ID),
        name: ActiveValue::Set(name.to_string()),
        description: ActiveValue::Set(description.map(str::to_string)),
        definition: ActiveValue::Set(definition.clone()),
        visibility: ActiveValue::Set("community".to_string()),
        author: ActiveValue::Set(Some(OFFICIAL_AUTHOR.to_string())),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    let row = Entity::insert(active)
        .on_conflict(
            OnConflict::columns([Column::TenantId, Column::Name])
                .update_columns([
                    Column::Description,
                    Column::Definition,
                    Column::Visibility,
                    Column::Author,
                    Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec_with_returning(conn)
        .await
        .internal()?;

    Ok(row.id)
}
