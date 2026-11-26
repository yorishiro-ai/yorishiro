pub use crate::models::_entities::iam_audit_events::{ActiveModel, Entity, Model};
use sea_orm::{ActiveValue::Set, DatabaseTransaction, QueryOrder, QuerySelect, entity::prelude::*};

use crate::error::{ResultExt, YorishiroError};
use crate::models::access_keys::AuthContext;

#[loco_rs::prelude::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::generated_id(this.id);
        this.created_at = crate::db::stamped_created_at(insert, this.created_at);
        Ok(this)
    }
}

impl Model {
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        use crate::models::{
            iam::access,
            iam::policy::{Action, Resource},
        };
        let txn = crate::db::begin_write(db).await.internal()?;
        access::require_iam_action(
            &txn,
            actor,
            Action::ListIamAuditEvents,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let events = Entity::find()
            .filter(
                crate::models::_entities::iam_audit_events::Column::TenantId.eq(actor.tenant_id),
            )
            .order_by_desc(crate::models::_entities::iam_audit_events::Column::CreatedAt)
            .order_by_desc(crate::models::_entities::iam_audit_events::Column::Id)
            .limit(params.limit())
            .offset(params.offset())
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
    action: crate::models::iam::policy::Action,
    detail: serde_json::Value,
) -> Result<(), YorishiroError> {
    ActiveModel {
        tenant_id: Set(actor.tenant_id),
        user_id: Set(Some(actor.user_id.ok_or(YorishiroError::Unauthenticated)?)),
        access_key_id: Set(actor.access_key_id),
        action: Set(action.name().to_owned()),
        detail: Set(detail),
        ..Default::default()
    }
    .insert(txn)
    .await
    .internal()?;
    Ok(())
}
