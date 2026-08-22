//! CRUD for `identity_templates`, the user-contributed schema template library.
//!
//! Distinct from `crate::templates` (the built-in templates shipped with the binary and served
//! from memory): these are tenant-scoped, DB-backed templates that a tenant's members create and
//! manage. Runs on `ctx.db` (the migration-role connection), matching the rest of the identity
//! surface: `identity_templates` has no RLS of its own, so every function here takes a
//! `tenant_id` and filters/checks visibility explicitly.

pub use super::_entities::identity_templates::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;
use sea_orm::{Condition, QueryOrder};
use serde::Serialize;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::MetaSchemaDefinition;

pub type IdentityTemplates = Entity;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Checks `!is_set()` rather than `is_unchanged()`: an `ActiveModel` built with
    /// `..Default::default()` leaves untouched fields `NotSet`, not `Unchanged`, and
    /// `is_unchanged()` only matches the latter. See `content_entities.rs`'s copy of this
    /// comment for where this was caught live.
    async fn before_save<C>(self, _db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if !insert && !self.updated_at.is_set() {
            let mut this = self;
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
            Ok(this)
        } else {
            Ok(self)
        }
    }
}

// implement your read-oriented logic here
impl Model {}

// implement your write-oriented logic here
impl ActiveModel {}

// implement your custom finders, selectors oriented logic here
impl Entity {}

/// A row from the tenant's DB-backed template library, with `definition` parsed.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateRecord {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub visibility: String,
    pub author: Option<String>,
    pub fork_of: Option<uuid::Uuid>,
    pub created_by: Option<uuid::Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<Model> for TemplateRecord {
    type Error = YorishiroError;

    fn try_from(model: Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: model.id,
            tenant_id: model.tenant_id,
            name: model.name,
            description: model.description,
            definition: serde_json::from_value(model.definition).internal()?,
            tags: model.tags,
            locale: model.locale,
            visibility: model.visibility,
            author: model.author,
            fork_of: model.fork_of,
            created_by: model.created_by,
            created_at: model.created_at.into(),
            updated_at: model.updated_at.into(),
        })
    }
}

/// Templates visible to `tenant_id`: its own templates plus any published with community
/// visibility.
fn visible_to(tenant_id: uuid::Uuid) -> Condition {
    Condition::any()
        .add(Column::TenantId.eq(tenant_id))
        .add(Column::Visibility.eq("community"))
}

/// Lists templates visible to `tenant_id`: its own templates plus any published with community
/// visibility (cross-tenant sharing; not yet reachable through the API, but the query already
/// honors it so nothing else needs to change when publishing ships).
pub async fn list_templates(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
) -> Result<Vec<TemplateRecord>, YorishiroError> {
    let rows = Entity::find()
        .filter(visible_to(tenant_id))
        .order_by_asc(Column::CreatedAt)
        .all(conn)
        .await
        .internal()?;

    rows.into_iter().map(TemplateRecord::try_from).collect()
}

/// Fetches a single template, allowed when it belongs to `tenant_id` or is community-visible.
pub async fn get_template(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    template_id: uuid::Uuid,
) -> Result<TemplateRecord, YorishiroError> {
    let row = Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(visible_to(tenant_id))
        .one(conn)
        .await
        .internal()?;

    match row {
        Some(row) => row.try_into(),
        None => Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        ))),
    }
}

/// Resolves a `template_id` as either a library template or a built-in, and says which.
///
/// A UUID can only mean the library; anything else can only mean a built-in. Parsing decides
/// which, so neither lookup runs against an id that could not name it, and a library miss
/// reports the library's own not-found rather than the built-in one.
///
/// The returned id is the origin to record: `Some` for a library template, whose later edits the
/// schema can then be told about, and `None` for a built-in, which has no row to point at.
pub async fn resolve_template_definition(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    template_id: &str,
) -> Result<(MetaSchemaDefinition, Option<uuid::Uuid>), YorishiroError> {
    match uuid::Uuid::parse_str(template_id) {
        Ok(id) => {
            let template = get_template(conn, tenant_id, id).await?;
            Ok((template.definition, Some(template.id)))
        }
        Err(_) => Ok((crate::templates::get_template(template_id)?, None)),
    }
}
