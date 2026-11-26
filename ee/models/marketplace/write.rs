//! Marketplace writes: ownership checks, rating validation, version numbering under lock and fork name collisions.
//! The inserts and updates themselves live in the parent module.

use crate::db;
use crate::edition::ee::models::marketplace;
use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;
use crate::models::iam::policy::Action;
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
/// Only the owning tenant may publish, and the insert assigns the version number.
pub(crate) async fn publish_version(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    request: PublishVersionRequest,
) -> Result<TemplateVersionRecord, YorishiroError> {
    let txn =
        super::authorized_transaction(&ctx.db, actor, Action::PublishMarketplaceVersion).await?;
    let tenant_id = actor.tenant_id;
    let user_id = actor.user_id;
    require_ownership(&txn, tenant_id, template_id).await?;

    // Serializing on the template makes concurrent publishes get consecutive numbers instead of colliding on one.
    db::lock_for_update(&txn, &format!("template-version:{template_id}"))
        .await
        .internal()?;

    let record = marketplace::insert_next_version(&txn, template_id, &request, user_id).await?;

    txn.commit().await.internal()?;

    Ok(record)
}

/// Records this tenant's review, replacing its previous one if it had left one.
///
/// `tenant_id` comes from the authenticated context, never from the request body.
pub(crate) async fn submit_review(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    request: SubmitReviewRequest,
) -> Result<TemplateReviewRecord, YorishiroError> {
    let txn =
        super::authorized_transaction(&ctx.db, actor, Action::CreateMarketplaceReview).await?;
    let tenant_id = actor.tenant_id;
    let user_id = actor.user_id;
    if !(1..=5).contains(&request.rating) {
        return Err(YorishiroError::ValidationFailed {
            message: "rating must be between 1 and 5".into(),
            details: Vec::new(),
            hint: "send an integer rating from 1 (worst) to 5 (best)".into(),
        });
    }

    // Not-found rather than forbidden, so a hidden template does not reveal that it exists.
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
/// The copy records `fork_of` and takes its definition from the chosen version, since the template row keeps moving as its owner edits it.
pub(crate) async fn fork_template(
    ctx: &AppContext,
    actor: &AuthContext,
    template_id: Uuid,
    version: Option<i32>,
) -> Result<Uuid, YorishiroError> {
    let txn =
        super::authorized_transaction(&ctx.db, actor, Action::ForkMarketplaceTemplate).await?;
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
        super::authorized_transaction(&ctx.db, actor, Action::UpdateMarketplaceVisibility).await?;
    let tenant_id = actor.tenant_id;
    require_ownership(&txn, tenant_id, template_id).await?;

    marketplace::update_visibility(&txn, template_id, visibility).await?;
    txn.commit().await.internal()
}

/// Rejects any operation on a template the caller's tenant does not own.
///
/// Reported as `NotFound` so a caller learns nothing about templates it cannot act on.
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
