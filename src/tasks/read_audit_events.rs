use loco_rs::prelude::*;
use loco_rs::task::Vars;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};

use crate::models::_entities::api_events::Column;

pub struct ReadAuditEvents;

#[async_trait]
impl Task for ReadAuditEvents {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "read_audit_events".into(),
            detail: "Print API trail rows as JSON lines: tenant_id=<uuid|none> [limit=<n>]; reads rows of retired tenants and rows no tenant owns".into(),
        }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let limit = vars
            .cli
            .get("limit")
            .map_or(Ok(100), |raw| raw.parse::<u64>())
            .map_err(|_| Error::Message("limit is not a number".into()))?;
        for row in rows(&ctx.db, vars.cli_arg("tenant_id")?, limit).await? {
            println!(
                "{}",
                serde_json::to_string(&crate::dtos::iam::ApiEventRecord::from(row))?
            );
        }
        Ok(())
    }
}

/// The newest trail rows of one tenant, or of no tenant when `scope` is `none`.
///
/// # Errors
/// Returns an error when `scope` is neither a UUID nor `none`, or the query fails.
pub async fn rows(
    db: &DatabaseConnection,
    scope: &str,
    limit: u64,
) -> Result<Vec<crate::models::api_events::Model>> {
    let mut query = crate::models::api_events::Entity::find();
    query = if scope == "none" {
        query.filter(Column::TenantId.is_null())
    } else {
        let tenant_id = scope
            .parse::<Uuid>()
            .map_err(|_| Error::Message("tenant_id is a UUID or none".into()))?;
        query.filter(Column::TenantId.eq(tenant_id))
    };
    query
        .order_by_desc(Column::CreatedAt)
        .order_by_desc(Column::Id)
        .limit(limit)
        .all(db)
        .await
        .map_err(Error::wrap)
}
