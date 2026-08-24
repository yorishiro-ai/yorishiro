//! Following an origin template: what has changed upstream, what merging would do, and doing it.
//!
//! Creating a schema from a template is not part of this: base owns `template_id` on `POST /api/schemas` and the `origin_*` columns.
//! This module owns the machinery that flows a template's edits into copies afterwards.
//!
//! These functions take both a schema connection and `ctx`, and the split is not incidental: the schema is workspace content, read over the RLS-scoped connection (the one an `Authorized` extractor's transaction holds), while `identity_templates` is control-plane data the request role holds no grant on and can only be reached through `ctx.db`.
//! Passing `ctx.db` for the schema side would bypass RLS; passing the RLS-scoped connection for the template side would fail with a permission error.

use loco_rs::app::AppContext;
use sea_orm::ConnectionTrait;
use uuid::Uuid;
use yorishiro_core::error::YorishiroError;
use yorishiro_core::metaschema::{MetaSchemaDefinition, VersioningDiff};
use yorishiro_core::models::content_schemas::{self, SchemaRecord};
use yorishiro_core::models::identity_templates;

use crate::services::merge::{self, MergePlan};

/// What following the origin template would do to this schema.
///
/// Reads the three definitions (the snapshot taken when the copy was made, the template as it stands now, and this schema) and classifies every field that differs.
/// Nothing is written.
///
/// Refuses rather than guesses when a piece is missing: a schema with no origin has nothing to follow, and one copied before snapshots were recorded has no ancestor.
/// Substituting the current template for the missing base would read every local addition as a conflict, which is worse than saying so.
pub async fn merge_preview(
    schema_conn: &impl ConnectionTrait,
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<MergePlan, YorishiroError> {
    let sides = merge_sides(schema_conn, ctx, tenant_id, workspace_id, schema_id).await?;
    Ok(merge::three_way(
        &sides.base,
        &sides.upstream,
        &sides.local.definition,
    ))
}

/// The three definitions a merge compares, resolved together because preview and apply need exactly the same set and must refuse on exactly the same grounds.
struct MergeSides {
    base: MetaSchemaDefinition,
    upstream: MetaSchemaDefinition,
    local: SchemaRecord,
    template_id: Uuid,
}

async fn merge_sides(
    schema_conn: &impl ConnectionTrait,
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<MergeSides, YorishiroError> {
    let schema = content_schemas::get_by_id(schema_conn, workspace_id, schema_id).await?;

    // `get_by_id` fetches any version, archived ones included: it is how a caller reads an old definition.
    // Merging into one is a different matter: `create_schema` archives whatever is currently active and installs the result as the new active version, so merging an archived version would resurrect an abandoned definition as the live one, and entities written against the current active version would find their schema replaced by an older lineage.
    // Refuse instead, and name the schema so the caller can look up the active version.
    if schema.status != "active" {
        return Err(YorishiroError::ValidationFailed {
            message: format!(
                "schema '{schema_id}' is {} and cannot be merged into",
                schema.status
            ),
            details: vec![],
            hint: format!(
                "merge the active version of '{}' instead; GET /api/schemas lists it",
                schema.name
            ),
        });
    }

    let Some(template_id) = schema.origin_template_id else {
        return Err(YorishiroError::ValidationFailed {
            message: format!("schema '{schema_id}' does not follow a template"),
            details: vec![],
            hint: "only a schema created from a template library entry can be merged; create \
                   one with POST /api/schemas and a template_id"
                .to_string(),
        });
    };

    let Some(base) = schema.origin_snapshot.clone() else {
        return Err(YorishiroError::ValidationFailed {
            message: format!("schema '{schema_id}' was copied before its merge base was recorded"),
            details: vec![],
            hint: "re-apply the template to this workspace to establish a base, or edit the \
                   schema directly"
                .to_string(),
        });
    };

    let template = identity_templates::get_template(&ctx.db, tenant_id, template_id).await?;

    Ok(MergeSides {
        base,
        upstream: template.definition,
        local: schema,
        template_id,
    })
}

/// Follows the origin template: writes the merged definition as the schema's next version.
///
/// Refuses a merge with conflicts, for the reason [`merge::apply_plan`] gives: a partially applied merge is a definition neither side asked for.
///
/// The result is a new version rather than an edit of the current one.
/// Every schema write goes through the same path, and taking upstream's changes is a schema change: entities written against the previous definition keep validating against the definition they were written against, which is what makes following a template safe to do on a workspace with data in it.
///
/// The new version's merge base is what upstream says *now*, not the merged result.
/// That is the point the next merge compares from.
pub async fn merge_apply(
    schema_conn: &impl ConnectionTrait,
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<(SchemaRecord, VersioningDiff), YorishiroError> {
    let sides = merge_sides(schema_conn, ctx, tenant_id, workspace_id, schema_id).await?;

    let plan = merge::three_way(&sides.base, &sides.upstream, &sides.local.definition);
    let merged = merge::apply_plan(&plan, &sides.upstream, &sides.local.definition)?;

    content_schemas::create_schema(
        schema_conn,
        tenant_id,
        workspace_id,
        merged,
        Some(sides.template_id),
        Some(sides.upstream),
    )
    .await
}
