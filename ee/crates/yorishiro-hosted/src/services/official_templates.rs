//! Publishes the community edition's built-in templates as official marketplace listings.
//! Bypasses `services::marketplace::publish_version`'s ownership check, since the seed has no authenticated tenant to check ownership against.
//! Invoked from a Loco task (`register_tasks`), not a request path.

use loco_rs::app::AppContext;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ConnectionTrait, EntityTrait, FromQueryResult, Statement, TransactionTrait,
};
use uuid::Uuid;
use yorishiro_core::db;
use yorishiro_core::error::{ResultExt, YorishiroError};
use yorishiro_core::models::_entities::identity_tenants;
use yorishiro_core::models::tenancy::INFRASTRUCTURE_TENANT_ID;

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

    for summary in yorishiro_core::templates::list_templates() {
        let definition = yorishiro_core::templates::get_template(&summary.id)?;
        let definition_json = serde_json::to_value(&definition).internal()?;

        let template_id = upsert_template(
            &ctx.db,
            &summary.id,
            summary.description.as_deref(),
            &definition_json,
        )
        .await?;

        // Compare against the newest version of any status, not just `stable`: publishing a
        // fresh version every run would otherwise walk the version number up forever while the
        // definition stayed the same.
        let latest = LatestVersion::find_by_statement(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT version, definition FROM identity_template_versions \
              WHERE template_id = $1 ORDER BY version DESC LIMIT 1",
            [template_id.into()],
        ))
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
        let request = crate::models::marketplace::PublishVersionRequest {
            definition: definition_json,
            changelog: Some(format!("Built-in template '{}'", summary.id)),
            status: "stable".to_string(),
        };
        // lock_for_update is transaction-scoped, so this needs its own txn rather than ctx.db.
        let txn = ctx.db.begin().await.internal()?;
        db::lock_for_update(&txn, &format!("template-version:{template_id}"))
            .await
            .internal()?;
        crate::models::marketplace::insert_next_version(&txn, template_id, &request, None).await?;
        txn.commit().await.internal()?;
    }

    Ok(outcome)
}

#[derive(FromQueryResult)]
struct LatestVersion {
    #[allow(dead_code)]
    version: i32,
    definition: serde_json::Value,
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
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
    }

    let row = Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO identity_templates \
                (tenant_id, name, description, definition, visibility, author) \
         VALUES ($1, $2, $3, $4, 'community', $5) \
         ON CONFLICT (tenant_id, name) DO UPDATE \
            SET description = EXCLUDED.description, \
                definition  = EXCLUDED.definition, \
                visibility  = 'community', \
                author      = EXCLUDED.author, \
                updated_at  = now() \
         RETURNING id",
        [
            OFFICIAL_TENANT_ID.into(),
            name.into(),
            description.into(),
            definition.clone().into(),
            OFFICIAL_AUTHOR.into(),
        ],
    ))
    .one(conn)
    .await
    .internal()?
    .ok_or_else(|| YorishiroError::Internal(anyhow::anyhow!("upsert did not return a row")))?;

    Ok(row.id)
}
