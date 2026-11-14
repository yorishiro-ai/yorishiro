pub use crate::models::_entities::iam_audit_events::{ActiveModel, Entity, Model};
use sea_orm::{ActiveValue::Set, DatabaseTransaction, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::AuthContext;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, db: &C, _: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        self.id = crate::db::sqlite_generated_id(db, self.id);
        Ok(self)
    }
}

impl Model {
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::{
            access, api_keys,
            iam_policy::{Action, Resource},
        };
        let txn = crate::db::begin_write(db).await.internal()?;
        access::require_iam_action(
            &txn,
            actor,
            api_keys::ApiKeyScope::Read,
            Action::IamAuditList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let current = api_keys::Model::current_context(&txn, actor).await?;
        access::require_audit(&current)?;
        let events = Entity::find()
            .filter(
                crate::models::_entities::iam_audit_events::Column::TenantId.eq(actor.tenant_id),
            )
            .order_by_desc(crate::models::_entities::iam_audit_events::Column::CreatedAt)
            .order_by_desc(crate::models::_entities::iam_audit_events::Column::Id)
            .limit(params.limit() as u64)
            .offset(params.offset() as u64)
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(events)
    }
}

pub(crate) async fn record(
    txn: &DatabaseTransaction,
    actor: &AuthContext,
    action: &str,
    detail: serde_json::Value,
) -> Result<(), YorishiroError> {
    ActiveModel {
        tenant_id: Set(actor.tenant_id),
        user_id: Set(actor.user_id.ok_or(YorishiroError::Unauthenticated)?),
        api_key_id: Set(actor.api_key_id),
        action: Set(action.to_owned()),
        detail: Set(detail),
        ..Default::default()
    }
    .insert(txn)
    .await
    .internal()?;
    Ok(())
}
