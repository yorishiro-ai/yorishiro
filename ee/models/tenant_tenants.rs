//! Usage counters for invoicing/dashboard display.

use crate::error::{ResultExt, YorishiroError};
use crate::models::_entities::{entity_entities, tenant_memberships, workspace_workspaces};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub(crate) struct TenantUsage {
    pub(crate) tenant_id: Uuid,
    pub(crate) workspace_count: i64,
    pub(crate) member_count: i64,
    pub(crate) entity_count: i64,
}

/// Computes usage counters for invoicing/dashboard display.
/// Runs over `ctx.db` (the admin/migration-role connection), since it aggregates across every workspace in a tenant and `entity_entities` only has a workspace-level RLS policy, not a tenant-wide one.
pub(crate) async fn compute_tenant_usage(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
) -> Result<TenantUsage, YorishiroError> {
    let workspace_count = workspace_workspaces::Entity::find()
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    let member_count = tenant_memberships::Entity::find()
        .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    // Filters on workspace_workspaces.tenant_id, a column entity_entities does not itself carry,
    // via the belongs_to relation entity_entities::Relation::IdentityWorkspaces already defines
    // (entity_entities.workspace_id -> workspace_workspaces.id).
    let entity_count = entity_entities::Entity::find()
        .inner_join(workspace_workspaces::Entity)
        .filter(workspace_workspaces::Column::TenantId.eq(tenant_id))
        .count(conn)
        .await
        .internal()?;

    Ok(TenantUsage {
        tenant_id,
        workspace_count: workspace_count as i64,
        member_count: member_count as i64,
        entity_count: entity_count as i64,
    })
}

/// Reads the current tenant administrative summary after revalidating the actor.
pub(crate) async fn overview_as(
    ctx: &loco_rs::app::AppContext,
    actor: &crate::models::api_keys::AuthContext,
) -> Result<crate::edition::ee::dtos::dashboard::TenantOverview, YorishiroError> {
    use crate::edition::ee::dtos::dashboard::TenantOverview;
    use crate::edition::ee::models::{tenant_billing, tenant_tenants};
    use crate::models::tenant_memberships;
    let current = crate::models::api_keys::Model::current_context(&ctx.db, actor).await?;
    crate::models::access::require_tenant_admin(&ctx.db, current.tenant_id, current.user_id)
        .await?;
    let tenant_id = current.tenant_id;
    let tenant = crate::models::tenant_tenants::Model::find_by_id(&ctx.db, tenant_id).await?;
    let billing = tenant_billing::get_billing(&ctx.db, tenant_id).await?;
    let usage = tenant_tenants::compute_tenant_usage(&ctx.db, tenant_id).await?;
    let active_plan = match crate::edition::ee::data::plan::PlanPolicy::current(ctx)? {
        crate::edition::ee::data::plan::PlanPolicy::Disabled => None,
        crate::edition::ee::data::plan::PlanPolicy::Enabled => {
            Some(crate::edition::ee::data::plan::Plan::for_tenant(ctx, tenant_id).await?)
        }
    };
    let effective_max_workspaces = crate::data::quotas::effective_limit(
        tenant.max_workspaces,
        active_plan.and_then(|plan| plan.caps().max_workspaces),
    );
    // The dashboard overview shows every member, not a page: it's a fixed-shape summary, not a browsable list with its own query params.
    // A tenant with more than MAX_LIST_LIMIT members sees a truncated list; flagged rather than silently accepted, since nothing here has measured how common that is.
    let members = tenant_memberships::list_members(
        &ctx.db,
        tenant_id,
        crate::models::pagination::ListParams {
            limit: crate::models::pagination::MAX_LIST_LIMIT,
            offset: 0,
        },
    )
    .await?;

    Ok(TenantOverview {
        tenant_id,
        plan: billing.and_then(|record| record.plan),
        active_plan,
        max_workspaces: tenant.max_workspaces,
        effective_max_workspaces,
        usage,
        members,
    })
}
