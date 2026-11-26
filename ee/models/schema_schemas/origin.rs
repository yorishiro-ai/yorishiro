//! Following an origin template: what has changed upstream, what merging would do, and doing it.
//!
//! Creating a schema from a template belongs to the community schema model.
//! This module flows a template's later edits into the copies.
//!
//! The functions take both a schema connection and `ctx`.
//! The schema is workspace content, read over the RLS-scoped connection.
//! `template_templates` is control-plane data the request role holds no grant on, so it is read through `ctx.db`.
//! Passing `ctx.db` for the schema side would bypass RLS, and passing the scoped connection for the template side would fail with a permission error.

use crate::error::{OptionExt, ResultExt, YorishiroError};
use crate::models::schema_schemas::metaschema::{MetaSchemaDefinition, VersioningDiff};
use crate::models::schema_schemas::{self, SchemaRecord};
use crate::models::template_templates;
use loco_rs::app::AppContext;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::merge::{self, MergePlan};

/// What following the origin template would do to this schema.
///
/// Reads the three definitions (the snapshot taken when the copy was made, the template as it stands now, and this schema) and classifies every field that differs.
/// Nothing is written.
///
/// Refuses when a piece is missing: a schema with no origin has nothing to follow.
pub(crate) async fn merge_preview(
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

/// The three definitions a merge compares, resolved together so preview and apply refuse on the same grounds.
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
    let schema = schema_schemas::get_by_id(schema_conn, workspace_id, schema_id).await?;

    // `create_schema` installs the merge result as the new active version, so merging an archived version would resurrect an abandoned definition.
    if schema.status != schema_schemas::SchemaStatus::Active {
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

    // The publisher takes this lock before changing the template and marking subscribers pending.
    // Holding it until commit prevents acknowledging a revision newer than the one read here.
    crate::db::lock_for_update(schema_conn, &format!("template-origin:{template_id}"))
        .await
        .internal()?;

    let base = schema
        .origin_snapshot
        .clone()
        .or_internal("linked schema has no merge base")?;

    let template = template_templates::get_template(&ctx.db, tenant_id, template_id).await?;

    Ok(MergeSides {
        base,
        upstream: template.definition,
        local: schema,
        template_id,
    })
}

/// Follows the origin template: writes the merged definition as the schema's next version.
///
/// Refuses a merge with conflicts, for the reason [`merge::apply_plan`] gives.
///
/// The result is a new version rather than an edit of the current one, so entities written against the previous definition keep validating against it.
///
/// The new version's merge base is the template as it stands now, not the merged result.
/// The next merge compares from there.
pub(crate) async fn merge_apply(
    schema_conn: &impl ConnectionTrait,
    ctx: &AppContext,
    tenant_id: Uuid,
    workspace_id: Uuid,
    schema_id: Uuid,
) -> Result<
    (
        SchemaRecord,
        VersioningDiff,
        schema_schemas::MergeDiffSummary,
    ),
    YorishiroError,
> {
    let sides = merge_sides(schema_conn, ctx, tenant_id, workspace_id, schema_id).await?;

    let plan = merge::three_way(&sides.base, &sides.upstream, &sides.local.definition);
    let merged = merge::apply_plan(&plan, &sides.upstream, &sides.local.definition)?;

    let (schema, diff) = schema_schemas::create_schema(
        schema_conn,
        tenant_id,
        workspace_id,
        merged,
        Some(sides.template_id),
        Some(sides.upstream),
    )
    .await?;

    Ok((schema, diff, plan.summary))
}

/// Opens the caller's read session and previews merging the schema's origin template.
///
/// # Errors
/// Returns the session, authorization or preview error.
pub(crate) async fn merge_preview_for_actor(
    ctx: &AppContext,
    actor: crate::models::access_keys::AuthContext,
    schema_id: Uuid,
) -> Result<MergePlan, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
        crate::models::iam::policy::Action::PreviewSchemaMerge,
        async |session| {
            // The schema is read on the scoped transaction, the template through `ctx.db`.
            merge_preview(
                session.txn(),
                ctx,
                session.ctx.tenant_id,
                session.workspace_id()?,
                schema_id,
            )
            .await
        },
    )
    .await
}

/// Opens the caller's write session and writes the merged definition as the next version.
///
/// # Errors
/// Returns the session, authorization or merge error.
pub(crate) async fn merge_apply_for_actor(
    ctx: &AppContext,
    actor: crate::models::access_keys::AuthContext,
    schema_id: Uuid,
) -> Result<crate::edition::ee::dtos::origin::MergeResponse, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::run(
        ctx,
        actor,
        crate::models::iam::access::WorkspaceTarget::Schema(schema_id),
        Some(crate::models::iam::policy::Action::ApplySchemaMerge),
        async |session| {
            let (schema, diff, summary) = merge_apply(
                session.txn(),
                ctx,
                session.ctx.tenant_id,
                session.workspace_id()?,
                schema_id,
            )
            .await?;
            Ok(crate::edition::ee::dtos::origin::MergeResponse {
                schema,
                diff,
                summary,
            })
        },
    )
    .await
}
