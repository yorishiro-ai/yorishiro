//! IAM role model: role administration and assuming a role for a temporary credential.

pub use crate::models::_entities::iam_roles::{ActiveModel, Column, Entity, Model};
use sea_orm::{ActiveValue::Set, QueryFilter, QueryOrder, QuerySelect, entity::prelude::*};
use serde::Serialize;

use crate::error::{ResultExt, YorishiroError};
use crate::models::api_keys::{ApiKeyScope, AuthContext};
use crate::models::iam_policy::{Action, Resource};
use crate::models::iam_trust_policy::{Caller, Principal, TrustPolicy};

pub const MIN_SESSION_SECONDS: i64 = 900;
pub const DEFAULT_SESSION_SECONDS: i64 = 3600;
pub const MAX_SESSION_SECONDS: i64 = 43_200;
pub const MAX_POLICIES_PER_ROLE: u64 = 20;

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

pub struct CreateRole {
    pub name: String,
    pub trust_policy: serde_json::Value,
    pub max_session_seconds: Option<i64>,
}

pub struct UpdateRole {
    pub name: Option<String>,
    pub trust_policy: Option<serde_json::Value>,
    pub max_session_seconds: Option<i64>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RoleSession {
    pub api_key_id: Uuid,
    pub role_id: Uuid,
    pub scope: ApiKeyScope,
    pub expires_at: chrono::DateTime<chrono::FixedOffset>,
    pub plaintext: String,
}

fn invalid(message: &'static str, hint: &'static str) -> YorishiroError {
    YorishiroError::ValidationFailed {
        message: message.into(),
        details: vec![],
        hint: hint.into(),
    }
}

fn clean_name(name: &str) -> Result<&str, YorishiroError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid(
            "role name must be between 1 and 100 characters",
            "provide a non-empty name of at most 100 characters",
        ));
    }
    Ok(name)
}

fn clean_max_session(value: Option<i64>) -> Result<i32, YorishiroError> {
    let seconds = value.unwrap_or(DEFAULT_SESSION_SECONDS);
    if !(DEFAULT_SESSION_SECONDS..=MAX_SESSION_SECONDS).contains(&seconds) {
        return Err(invalid(
            "max_session_seconds must be between 3600 and 43200",
            "choose a maximum session of 1 to 12 hours",
        ));
    }
    i32::try_from(seconds).internal()
}

fn unique_name_conflict(error: DbErr) -> YorishiroError {
    if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
        YorishiroError::Conflict {
            message: "an IAM role with this name already exists".into(),
        }
    } else {
        YorishiroError::Internal(error.into())
    }
}

async fn validated_trust(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
    document: &serde_json::Value,
) -> Result<TrustPolicy, YorishiroError> {
    use crate::models::_entities::{api_keys, tenant_memberships};
    let trust = TrustPolicy::parse(&serde_json::to_string(document).internal()?)?;
    for principal in trust.principals() {
        let known = match principal {
            Principal::Tenant => true,
            Principal::User { user_id } => {
                if crate::models::iam_tenant_roots::Model::contains(txn, tenant_id, user_id).await?
                {
                    return Err(invalid(
                        "the tenant root cannot assume roles",
                        "name a tenant user, a group or a service key",
                    ));
                }
                tenant_memberships::Entity::find()
                    .filter(tenant_memberships::Column::TenantId.eq(tenant_id))
                    .filter(tenant_memberships::Column::UserId.eq(user_id))
                    .one(txn)
                    .await
                    .internal()?
                    .is_some()
            }
            Principal::Group { group_id } => {
                crate::models::iam_groups::Model::find(txn, tenant_id, group_id)
                    .await?
                    .is_some()
            }
            Principal::ServiceKey { api_key_id } => api_keys::Entity::find_by_id(api_key_id)
                .filter(api_keys::Column::TenantId.eq(tenant_id))
                .filter(api_keys::Column::UserId.is_null())
                .filter(api_keys::Column::RoleId.is_null())
                .filter(api_keys::Column::RevokedAt.is_null())
                .one(txn)
                .await
                .internal()?
                .is_some(),
        };
        if !known {
            return Err(invalid(
                "a trust policy principal does not belong to this tenant",
                "name users, groups and service keys of the same tenant",
            ));
        }
    }
    Ok(trust)
}

impl Model {
    pub(crate) async fn list_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        params: crate::models::pagination::ListParams,
    ) -> Result<Vec<Self>, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            Action::RoleList,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let rows = Entity::find()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .order_by_asc(Column::Id)
            .limit(params.limit() as u64)
            .offset(params.offset() as u64)
            .all(&txn)
            .await
            .internal()?;
        txn.commit().await.internal()?;
        Ok(rows)
    }

    pub(crate) async fn get_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        id: Uuid,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            Action::RoleGet,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let row = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        txn.commit().await.internal()?;
        Ok(row)
    }

    pub(crate) async fn create_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        input: CreateRole,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            Action::RoleCreate,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let name = clean_name(&input.name)?.to_owned();
        let max_session_seconds = clean_max_session(input.max_session_seconds)?;
        validated_trust(&txn, actor.tenant_id, &input.trust_policy).await?;
        let role = ActiveModel {
            tenant_id: Set(actor.tenant_id),
            name: Set(name),
            trust_policy: Set(input.trust_policy),
            max_session_seconds: Set(max_session_seconds),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(unique_name_conflict)?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_create",
            serde_json::json!({"role_id": role.id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(role)
    }

    pub(crate) async fn update_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        id: Uuid,
        input: UpdateRole,
    ) -> Result<Self, YorishiroError> {
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            Action::RoleUpdate,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        let current = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        let mut active: ActiveModel = current.into();
        if let Some(name) = &input.name {
            active.name = Set(clean_name(name)?.to_owned());
        }
        if let Some(trust) = input.trust_policy {
            validated_trust(&txn, actor.tenant_id, &trust).await?;
            active.trust_policy = Set(trust);
        }
        if input.max_session_seconds.is_some() {
            active.max_session_seconds = Set(clean_max_session(input.max_session_seconds)?);
        }
        active.updated_at = Set(chrono::Utc::now().into());
        let role = active.update(&txn).await.map_err(unique_name_conflict)?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_update",
            serde_json::json!({"role_id": id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(role)
    }

    pub(crate) async fn delete_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        id: Uuid,
    ) -> Result<(), YorishiroError> {
        use crate::models::_entities::api_keys;
        let txn = crate::db::begin_write(db).await.internal()?;
        crate::models::access::require_iam_action(
            &txn,
            actor,
            ApiKeyScope::Read,
            Action::RoleDelete,
            Resource::Tenant {
                tenant_id: actor.tenant_id,
            },
        )
        .await?;
        if Self::find(&txn, actor.tenant_id, id).await?.is_none() {
            return Err(YorishiroError::not_found("IAM role was not found"));
        }
        api_keys::Entity::revoke_matching(
            &txn,
            sea_orm::Condition::all()
                .add(api_keys::Column::TenantId.eq(actor.tenant_id))
                .add(api_keys::Column::RoleId.eq(id)),
        )
        .await?;
        Entity::delete_many()
            .filter(Column::TenantId.eq(actor.tenant_id))
            .filter(Column::Id.eq(id))
            .exec(&txn)
            .await
            .internal()?;
        crate::models::iam_audit_events::record(
            &txn,
            actor,
            "role_delete",
            serde_json::json!({"role_id": id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(())
    }

    pub(crate) async fn assume_as(
        db: &DatabaseConnection,
        actor: &AuthContext,
        id: Uuid,
        scope: Option<ApiKeyScope>,
        duration_seconds: Option<i64>,
    ) -> Result<RoleSession, YorishiroError> {
        use crate::models::_entities::{api_keys, iam_group_memberships};
        let txn = crate::db::begin_write(db).await.internal()?;
        let evaluation =
            crate::models::access::IamEvaluation::load_for_assumption(&txn, actor).await?;
        let denied = || YorishiroError::ScopeInsufficient {
            message: "this role cannot be assumed by the caller".into(),
            hint: "the role trust policy and a role_assume permission must both allow it".into(),
        };
        let role = Self::find(&txn, actor.tenant_id, id)
            .await?
            .ok_or_else(|| YorishiroError::not_found("IAM role was not found"))?;
        let caller_key = api_keys::Entity::find_by_id(actor.api_key_id)
            .one(&txn)
            .await
            .internal()?
            .ok_or(YorishiroError::Unauthenticated)?;
        let trust = TrustPolicy::parse(&serde_json::to_string(&role.trust_policy).internal()?)?;
        let group_ids = match actor.user_id {
            Some(user_id) => iam_group_memberships::Entity::find()
                .filter(iam_group_memberships::Column::TenantId.eq(actor.tenant_id))
                .filter(iam_group_memberships::Column::UserId.eq(user_id))
                .all(&txn)
                .await
                .internal()?
                .into_iter()
                .map(|membership| membership.group_id)
                .collect(),
            None => Vec::new(),
        };
        let trusted = trust.permits(&Caller {
            user_id: actor.user_id,
            group_ids: &group_ids,
            api_key_id: actor.api_key_id,
        });
        if actor.user_id.is_some() {
            evaluation
                .require(
                    Action::RoleAssume,
                    Resource::Role {
                        tenant_id: actor.tenant_id,
                        role_id: id,
                    },
                )
                .await
                .map_err(|_| denied())?;
        } else if !trust.names_service_key(actor.api_key_id) {
            return Err(denied());
        }
        if !trusted {
            return Err(denied());
        }

        let caller_scope = ApiKeyScope::from_db_str(&caller_key.scope)
            .ok_or_else(|| YorishiroError::Internal("unknown api key scope".into()))?;
        let scope = scope.unwrap_or(caller_scope);
        if scope > caller_scope {
            return Err(YorishiroError::ScopeInsufficient {
                message: "a role session cannot exceed the scope of the credential that assumes it"
                    .into(),
                hint: "request a scope no higher than the calling credential".into(),
            });
        }
        let duration = duration_seconds.unwrap_or(DEFAULT_SESSION_SECONDS);
        if !(MIN_SESSION_SECONDS..=i64::from(role.max_session_seconds)).contains(&duration) {
            return Err(invalid(
                "duration_seconds is outside the allowed range",
                "choose between 900 seconds and the role's maximum session",
            ));
        }
        let mut expires_at: chrono::DateTime<chrono::FixedOffset> =
            (crate::data::time::now() + chrono::Duration::seconds(duration)).into();
        if let Some(limit) = caller_key.expires_at {
            expires_at = expires_at.min(limit);
        }
        let session = api_keys::Entity::create_role_session(
            &txn,
            actor.tenant_id,
            actor.user_id,
            id,
            scope,
            &role.name,
            expires_at,
        )
        .await?;
        crate::models::iam_audit_events::record_assumption(
            &txn,
            actor,
            serde_json::json!({"role_id": id, "session_api_key_id": session.id}),
        )
        .await?;
        txn.commit().await.internal()?;
        Ok(RoleSession {
            api_key_id: session.id,
            role_id: id,
            scope,
            expires_at,
            plaintext: session.plaintext,
        })
    }

    pub(crate) async fn find(
        txn: &sea_orm::DatabaseTransaction,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Self>, YorishiroError> {
        Entity::find_by_id(id)
            .filter(Column::TenantId.eq(tenant_id))
            .one(txn)
            .await
            .internal()
    }
}
