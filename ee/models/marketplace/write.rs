//! Decisions about the template marketplace: ownership, status/rating validation, version numbering under lock, and how a fork's name collision is reported.
//!
//! Read authorization lives in the actor-aware operations in `models::marketplace`.
//! This module holds only the writes, calling into `models::marketplace` for the insert/update itself once a decision allows it.

use crate::db;
use crate::edition::ee::models::marketplace;
use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::AuthContext;
use crate::models::iam_policy::Action;
use loco_rs::app::AppContext;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::{
    InsertForkOutcome, PublishVersionRequest, SubmitReviewRequest, TemplateReviewRecord,
    TemplateVersionRecord,
};
use crate::models::template_templates::TemplateVisibility;

/// Publishes the next version of a template.
///
/// Only the owning tenant may publish, and the version number is assigned in the insert itself rather than taken from the caller: letting a client choose it invites gaps and collisions in a sequence other tenants read as history.
pub(crate) async fn publish_version(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    request: PublishVersionRequest,
) -> Result<TemplateVersionRecord, YorishiroError> {
    let txn = super::authorized_transaction(&ctx.db, actor, Action::MarketplacePublish).await?;
    let tenant_id = actor.tenant_id;
    let user_id = actor.user_id;
    require_ownership(&txn, tenant_id, template_id).await?;

    // The insert reads `max(version) + 1` in the same statement it writes, and at READ COMMITTED Postgres locks no range for the rows that do not exist yet, so two concurrent publishes of one template both read the same maximum and both try to write the same next version.
    // Serializing on the template turns that into what the caller expects: both succeed, with consecutive numbers.
    // `db::lock_for_update` is transaction-scoped, so it releases on commit or rollback with no separate connection to leak.
    db::lock_for_update(&txn, &format!("template-version:{template_id}"))
        .await
        .internal()?;

    let record = marketplace::insert_next_version(&txn, template_id, &request, user_id).await?;

    txn.commit().await.internal()?;

    Ok(record)
}

/// Records this tenant's review, replacing its previous one if it had left one.
///
/// `tenant_id` comes from the authenticated context, never from the request body: taking it from input would let any caller review as any tenant, which is the whole value of a rating.
pub(crate) async fn submit_review(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    request: SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    let txn =
        super::authorized_transaction(&ctx.db, actor, Action::MarketplaceReviewSubmit).await?;
    let tenant_id = actor.tenant_id;
    let user_id = actor.user_id;
    if !(1..=5).contains(&request.rating) {
        return Err(YorishiroError::ValidationFailed {
            message: "rating must be between 1 and 5".into(),
            details: Vec::new(),
            hint: "send an integer rating from 1 (worst) to 5 (best)".into(),
        });
    }

    // Reviewing a template nobody can see is meaningless, and would leak that it exists.
    if !marketplace::is_visible(&txn, tenant_id, template_id).await? {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }

    let record =
        marketplace::upsert_review(&txn, tenant_id, template_id, user_id, &request).await?;
    txn.commit().await.internal()?;
    Ok(record)
}

/// Copies a published version of someone else's template into the caller's own library.
///
/// The copy records `fork_of`, and takes the definition from the *version* rather than the template row: the template keeps moving as its owner edits it, so forking "the template" would install whatever it happened to be at that instant rather than the version chosen.
pub(crate) async fn fork_template(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Uuid, YorishiroError> {
    let txn = super::authorized_transaction(&ctx.db, actor, Action::MarketplaceFork).await?;
    let tenant_id = actor.tenant_id;
    let user_id = actor.user_id;
    let Some(source) = marketplace::find_fork_source(&txn, tenant_id, template_id).await? else {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    };

    let Some(definition) =
        marketplace::find_forkable_definition(&txn, template_id, version).await?
    else {
        return Err(YorishiroError::not_found(
            "no published version of this template is available to fork",
        ));
    };

    let name = source.name.clone();
    let result =
        match marketplace::insert_fork(&txn, tenant_id, &source, &definition, template_id, user_id)
            .await?
        {
            InsertForkOutcome::Created(id) => Ok(id),
            InsertForkOutcome::NameTaken => Err(YorishiroError::Conflict {
                message: format!("this tenant already has a template named '{name}'"),
            }),
        };
    let id = result?;
    txn.commit().await.internal()?;
    Ok(id)
}

/// Sets a template's marketplace visibility.
/// Only its owning tenant may.
pub(crate) async fn set_visibility(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    visibility: TemplateVisibility,
) -> Result<(), YorishiroError> {
    let txn =
        super::authorized_transaction(&ctx.db, actor, Action::MarketplaceVisibilityUpdate).await?;
    let tenant_id = actor.tenant_id;
    require_ownership(&txn, tenant_id, template_id).await?;

    marketplace::update_visibility(&txn, template_id, visibility).await?;
    txn.commit().await.internal()
}

/// Rejects any operation on a template the caller's tenant does not own.
///
/// Reported as NotFound rather than Forbidden: a caller that cannot act on a template should not learn it exists from the difference between the two.
async fn require_ownership(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<(), YorishiroError> {
    if !marketplace::is_owned_by(conn, tenant_id, template_id).await? {
        return Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )));
    }
    Ok(())
}
