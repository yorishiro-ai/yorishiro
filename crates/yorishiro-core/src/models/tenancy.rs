use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::metaschema::MetaSchemaDefinition;
use crate::services::auth::ApiKeyScope;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TenantRecord {
    pub id: Uuid,
    pub name: String,
    pub plan: Option<String>,
    pub max_workspaces: Option<i32>,
    pub stripe_customer_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct WorkspaceRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub max_entities: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Mirrors the `identity.tenant_memberships.role` check constraint
/// (`owner`/`admin`/`member`/`viewer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl MembershipRole {
    pub(crate) fn as_db_str(self) -> &'static str {
        match self {
            MembershipRole::Owner => "owner",
            MembershipRole::Admin => "admin",
            MembershipRole::Member => "member",
            MembershipRole::Viewer => "viewer",
        }
    }

    pub(crate) fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(MembershipRole::Owner),
            "admin" => Some(MembershipRole::Admin),
            "member" => Some(MembershipRole::Member),
            "viewer" => Some(MembershipRole::Viewer),
            _ => None,
        }
    }

    /// The highest API key scope a member with this role may be issued. Enforced at key
    /// issuance time (see `admin create-api-key --user`), not re-checked afterward -- this
    /// mirrors how a key's scope is otherwise fixed for its lifetime until revoked.
    pub fn max_scope(self) -> ApiKeyScope {
        match self {
            MembershipRole::Owner | MembershipRole::Admin => ApiKeyScope::Schema,
            MembershipRole::Member => ApiKeyScope::Write,
            MembershipRole::Viewer => ApiKeyScope::Read,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MembershipRecord {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub role: MembershipRole,
}

#[derive(Debug, Clone, Serialize)]
pub struct InviteRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub email: String,
    pub role: MembershipRole,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// A user-contributed schema template stored in `identity.templates`. Distinct from the
/// built-in templates in `crate::templates` (shipped with the binary, served from memory):
/// this is the tenant-scoped, DB-backed template library.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TemplateRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub visibility: String,
    pub author: Option<String>,
    pub fork_of: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for creating a new template. `visibility` is not settable here -- every template
/// starts as tenant-private (`visibility = 'tenant'`); community publishing is a future,
/// separate operation once that workflow is designed (see the migration's comment).
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CreateTemplateInput {
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

/// Input for updating an existing template. Every field is optional; `None` leaves the
/// existing value unchanged.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdateTemplateInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}
