use sqlx::{PgPool, Row};
use uuid::Uuid;
use yorishiro_core::ResultExt;
use yorishiro_core::db;
use yorishiro_core::error::YorishiroError;

/// The tenant that owns the officially published templates.
///
/// A fixed id rather than a lookup by name, so re-running the seed finds the same row and a
/// deployment that renames it does not end up with two publishers.
pub const OFFICIAL_TENANT_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001);

/// Shown as the listing's author. `identity.templates.author` is free text, so this does not
/// require a user account.
pub const OFFICIAL_AUTHOR: &str = "Yorishiro";

const OFFICIAL_TENANT_NAME: &str = "Yorishiro Official";

/// What one seeding run did. Callers report it; nothing branches on it.
#[derive(Debug, Default)]
pub struct SeedOutcome {
    pub published: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Publishes the community edition's built-in templates as official marketplace listings.
///
/// The publisher is a tenant row with **no members and no workspaces**: `identity.templates`
/// requires a `tenant_id`, and the marketplace scopes ownership by it, so a listing has to
/// belong to some tenant. Nobody can log into this one (there is no membership to log in
/// through) and it holds no data of its own. It exists to satisfy the foreign key and to give
/// official listings a stable owner, not to be used.
///
/// Idempotent, and safe to run on every deployment: a template is matched by
/// `(tenant_id, name)`, and a new version is published only when the built-in definition
/// differs from the latest one already published. Re-running with unchanged built-ins writes
/// nothing.
pub async fn seed_official_templates(pool: &PgPool) -> Result<SeedOutcome, YorishiroError> {
    ensure_official_tenant(pool).await?;

    let mut outcome = SeedOutcome::default();

    for summary in yorishiro_core::templates::list_templates() {
        let definition = yorishiro_core::templates::get_template(&summary.id)?;
        let definition_json = serde_json::to_value(&definition).internal()?;

        let template_id = upsert_template(
            pool,
            &summary.id,
            summary.description.as_deref(),
            &definition_json,
        )
        .await?;

        // Compare against the newest version of any status, not just `stable`: publishing a
        // fresh version every run would otherwise walk the version number up forever while the
        // definition stayed the same.
        let latest: Option<(i32, serde_json::Value)> = sqlx::query(
            "SELECT version, definition FROM identity.template_versions \
             WHERE template_id = $1 ORDER BY version DESC LIMIT 1",
        )
        .bind(template_id)
        .fetch_optional(pool)
        .await
        .internal()?
        .map(|row| {
            Ok::<_, YorishiroError>((
                row.try_get("version").internal()?,
                row.try_get("definition").internal()?,
            ))
        })
        .transpose()?;

        match latest {
            Some((_, published)) if published == definition_json => {
                outcome.unchanged.push(summary.id.clone());
                continue;
            }
            Some(_) => outcome.updated.push(summary.id.clone()),
            None => outcome.published.push(summary.id.clone()),
        }

        // Same read-then-write race as `marketplace::publish_version`, and the same remedy: the
        // version is read by `max(version) + 1` inside the inserting statement, which locks no
        // range at READ COMMITTED. Two deployments seeding at once (a rolling restart is
        // enough) would otherwise both compute the same number and one would fail on
        // `UNIQUE (template_id, version)`.
        //
        // `stable` rather than `draft`: a draft is visible only to its owning tenant, and this
        // tenant has no members to view it. An official template that nobody can see is the
        // same as not publishing it.
        let mut tx = pool.begin().await.internal()?;
        db::lock_for_update(&mut tx, &format!("template-version:{template_id}"))
            .await
            .internal()?;
        sqlx::query(
            "INSERT INTO identity.template_versions \
                    (template_id, version, definition, changelog, status, created_by) \
             SELECT $1, COALESCE(max(v.version), 0) + 1, $2, $3, 'stable', NULL \
               FROM identity.template_versions v WHERE v.template_id = $1",
        )
        .bind(template_id)
        .bind(&definition_json)
        .bind(format!("Built-in template '{}'", summary.id))
        .execute(&mut *tx)
        .await
        .internal()?;
        tx.commit().await.internal()?;
    }

    Ok(outcome)
}

async fn ensure_official_tenant(pool: &PgPool) -> Result<(), YorishiroError> {
    // Written directly rather than through `create_tenant`, which enforces
    // `YORISHIRO_MAX_TENANTS`. The publisher is infrastructure, not a customer, and a
    // deployment sitting at its tenant cap still needs its official templates.
    sqlx::query(
        "INSERT INTO identity.tenants (id, name) VALUES ($1, $2) \
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(OFFICIAL_TENANT_ID)
    .bind(OFFICIAL_TENANT_NAME)
    .execute(pool)
    .await
    .internal()?;
    Ok(())
}

/// Creates the template row, or refreshes the description/definition of the existing one.
/// `identity.templates` is unique on `(tenant_id, name)`, which is what makes this idempotent.
async fn upsert_template(
    pool: &PgPool,
    name: &str,
    description: Option<&str>,
    definition: &serde_json::Value,
) -> Result<Uuid, YorishiroError> {
    let row = sqlx::query(
        "INSERT INTO identity.templates \
                (tenant_id, name, description, definition, visibility, author) \
         VALUES ($1, $2, $3, $4, 'community', $5) \
         ON CONFLICT (tenant_id, name) DO UPDATE \
            SET description = EXCLUDED.description, \
                definition  = EXCLUDED.definition, \
                visibility  = 'community', \
                author      = EXCLUDED.author, \
                updated_at  = now() \
         RETURNING id",
    )
    .bind(OFFICIAL_TENANT_ID)
    .bind(name)
    .bind(description)
    .bind(definition)
    .bind(OFFICIAL_AUTHOR)
    .fetch_one(pool)
    .await
    .internal()?;

    row.try_get("id").internal()
}

#[cfg(test)]
#[path = "../../tests/services/official_templates.rs"]
mod tests;
