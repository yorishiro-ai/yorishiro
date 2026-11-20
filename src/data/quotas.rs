//! Operation-time quota limits supplied by the active edition, and how they combine with operator-set limits.

/// Additional operation-time quotas, independent of persisted operator limits.
#[derive(Clone, Copy, Default)]
pub(crate) struct Quotas {
    pub(crate) workspaces: Option<i32>,
    pub(crate) entities: Option<i32>,
}

pub(crate) async fn for_tenant(
    ctx: &loco_rs::app::AppContext,
    tenant_id: uuid::Uuid,
) -> Result<Quotas, crate::YorishiroError> {
    Ok(crate::quota_override(ctx, tenant_id)
        .await?
        .unwrap_or_default())
}

pub(crate) fn effective_limit(manual: Option<i32>, policy: Option<i32>) -> Option<i32> {
    match (manual, policy) {
        (Some(manual), Some(policy)) => Some(manual.min(policy)),
        (manual, policy) => manual.or(policy),
    }
}
