//! Decisions about the template marketplace: ownership, status/rating validation, version
//! numbering under lock, and how a fork's name collision is reported.
//! Ported from master's `ee/crates/yorishiro-hosted/src/services/marketplace.rs`.
//!
//! The pure reads (`list_marketplace`, `list_versions`, `list_reviews`) have no decision attached
//! and live in `models::marketplace` alone; `controllers::marketplace` calls them directly.
//! This module holds only the writes, calling into `models::marketplace` for the insert/update
//! itself once a decision allows it.

use loco_rs::app::AppContext;
use sea_orm::TransactionTrait;
use uuid::Uuid;
use yorishiro_core::db;
use yorishiro_core::error::{ResultExt, YorishiroError};

use crate::models::marketplace::{
    self, InsertForkOutcome, PublishVersionRequest, SubmitReviewRequest, TemplateReviewRecord,
    TemplateVersionRecord,
};

fn validate_status(status: &str) -> Result<(), YorishiroError> {
    if matches!(status, "draft" | "pre" | "stable") {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: format!("unknown publish status '{status}'"),
            details: Vec::new(),
            hint: "use one of: draft, pre, stable".into(),
        })
    }
}

/// Publishes the next version of a template.
///
/// Only the owning tenant may publish, and the version number is assigned in the insert itself
/// rather than taken from the caller: letting a client choose it invites gaps and collisions in a
/// sequence other tenants read as history.
pub async fn publish_version(
    ctx: &AppContext,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: PublishVersionRequest,
) -> Result<TemplateVersionRecord, YorishiroError> {
    validate_status(&request.status)?;
    require_ownership(ctx, tenant_id, template_id).await?;

    // The insert reads `max(version) + 1` in the same statement it writes, and at READ COMMITTED
    // Postgres locks no range for the rows that do not exist yet, so two concurrent publishes of
    // one template both read the same maximum and both try to write the same next version.
    // The unique index on `(template_id, version)` catches it, which is why this was never
    // corruption, but the loser got an opaque 500 for doing nothing wrong.
    //
    // Serializing on the template turns that into what the caller expects: both succeed, with
    // consecutive numbers. `db::lock_for_update` is transaction-scoped, so it releases on commit
    // or rollback with no separate connection to leak (see `controllers::stripe`'s doc comment
    // for the shape this replaced).
    let txn = ctx.db.begin().await.internal()?;
    db::lock_for_update(&txn, &format!("template-version:{template_id}"))
        .await
        .internal()?;

    let record = marketplace::insert_next_version(&txn, template_id, &request, user_id).await?;

    txn.commit().await.internal()?;

    Ok(record)
}

/// Records this tenant's review, replacing its previous one if it had left one.
///
/// `tenant_id` comes from the authenticated context, never from the request body: taking it from
/// input would let any caller review as any tenant, which is the whole value of a rating.
pub async fn submit_review(
    ctx: &AppContext,
    tenant_id: Uuid,
    template_id: Uuid,
    user_id: Option<Uuid>,
    request: SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    if !(1..=5).contains(&request.rating) {
        return Err(YorishiroError::ValidationFailed {
            message: "rating must be between 1 and 5".into(),
            details: Vec::new(),
            hint: "send an integer rating from 1 (worst) to 5 (best)".into(),
        });
    }

    // Reviewing a template nobody can see is meaningless, and would leak that it exists.
    if !marketplace::is_visible(&ctx.db, tenant_id, template_id).await? {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }

    marketplace::upsert_review(&ctx.db, tenant_id, template_id, user_id, &request).await
}

/// Copies a published version of someone else's template into the caller's own library.
///
/// The copy records `fork_of`, and takes the definition from the *version* rather than the
/// template row: the template keeps moving as its owner edits it, so forking "the template"
/// would install whatever it happened to be at that instant rather than the version chosen.
pub async fn fork_template(
    ctx: &AppContext,
    tenant_id: Uuid,
    template_id: Uuid,
    version: Option<i32>,
    user_id: Option<Uuid>,
) -> Result<Uuid, YorishiroError> {
    let Some(source) = marketplace::find_fork_source(&ctx.db, tenant_id, template_id).await? else {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    };

    let Some(definition) =
        marketplace::find_forkable_definition(&ctx.db, template_id, version).await?
    else {
        return Err(YorishiroError::not_found(
            "no published version of this template is available to fork",
        ));
    };

    let name = source.name.clone();
    match marketplace::insert_fork(
        &ctx.db,
        tenant_id,
        &source,
        &definition,
        template_id,
        user_id,
    )
    .await?
    {
        InsertForkOutcome::Created(id) => Ok(id),
        InsertForkOutcome::NameTaken => Err(YorishiroError::Conflict {
            message: format!("this tenant already has a template named '{name}'"),
        }),
    }
}

/// Sets a template's marketplace visibility.
/// Only its owning tenant may.
pub async fn set_visibility(
    ctx: &AppContext,
    tenant_id: Uuid,
    template_id: Uuid,
    visibility: &str,
) -> Result<(), YorishiroError> {
    if !matches!(visibility, "tenant" | "community") {
        return Err(YorishiroError::ValidationFailed {
            message: format!("unknown visibility '{visibility}'"),
            details: Vec::new(),
            hint: "use 'tenant' to keep it private or 'community' to list it".into(),
        });
    }
    require_ownership(ctx, tenant_id, template_id).await?;

    marketplace::update_visibility(&ctx.db, template_id, visibility).await
}

/// Rejects any operation on a template the caller's tenant does not own.
///
/// Reported as NotFound rather than Forbidden: a caller that cannot act on a template should not
/// learn it exists from the difference between the two.
async fn require_ownership(
    ctx: &AppContext,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<(), YorishiroError> {
    if !marketplace::is_owned_by(&ctx.db, tenant_id, template_id).await? {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }
    Ok(())
}
