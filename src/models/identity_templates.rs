//! CRUD for `identity_templates`, the user-contributed schema template library.
//!
//! Distinct from `crate::templates` (the built-in templates shipped with the binary and served from memory): these are tenant-scoped, DB-backed templates that a tenant's members create and manage.
//! Runs on `ctx.db` (the migration-role connection): `identity_templates` has no RLS of its own, so every function here takes a `tenant_id` and filters/checks visibility explicitly.

pub use super::_entities::identity_templates::{ActiveModel, Column, Entity, Model};
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, Condition, QueryOrder, QuerySelect};
use serde::Serialize;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::MetaSchemaDefinition;

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    /// Stamps `updated_at` on every update whose caller didn't already set it explicitly.
    /// Checks `!is_set()` rather than `is_unchanged()`: an `ActiveModel` built with `..Default::default()` leaves untouched fields `NotSet`, not `Unchanged`, and `is_unchanged()` only matches the latter.
    ///
    /// `id` has a `uuidv7()` column default on PostgreSQL and no default on SQLite; see `crate::db::sqlite_generated_id`.
    async fn before_save<C>(self, db: &C, insert: bool) -> std::result::Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        let mut this = self;
        this.id = crate::db::sqlite_generated_id(db, this.id);
        if !insert && !this.updated_at.is_set() {
            this.updated_at = sea_orm::ActiveValue::Set(chrono::Utc::now().into());
        }
        Ok(this)
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

/// Templates visible to `tenant_id`: its own templates plus any published with community visibility.
fn visible_to(tenant_id: uuid::Uuid) -> Condition {
    Condition::any()
        .add(Column::TenantId.eq(tenant_id))
        .add(Column::Visibility.eq("community"))
}

/// Lists templates visible to `tenant_id`: its own templates plus any published with community visibility.
pub async fn list_templates(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    page: super::pagination::ListParams,
) -> Result<Vec<TemplateRecord>, YorishiroError> {
    let rows = Entity::find()
        .filter(visible_to(tenant_id))
        .order_by_asc(Column::CreatedAt)
        .limit(page.limit() as u64)
        .offset(page.offset() as u64)
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
/// A UUID can only mean the library; anything else can only mean a built-in.
/// Parsing decides which, so neither lookup runs against an id that could not name it, and a library miss reports the library's own not-found rather than the built-in one.
///
/// The returned id is the origin to record: `Some` for a library template, whose later edits the schema can then be told about, and `None` for a built-in, which has no row to point at.
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

/// Input for creating a new template.
/// `visibility` is not settable here: every template starts as tenant-private.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateTemplateInput {
    pub name: String,
    pub description: Option<String>,
    pub definition: MetaSchemaDefinition,
    #[serde(default)]
    pub tags: Vec<String>,
    pub locale: Option<String>,
    pub author: Option<String>,
}

/// Input for updating an existing template.
/// Every field is optional; `None` leaves the existing value unchanged.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateTemplateInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub definition: Option<MetaSchemaDefinition>,
    pub tags: Option<Vec<String>>,
    pub locale: Option<String>,
}

pub async fn create_template(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    created_by: Option<uuid::Uuid>,
    input: CreateTemplateInput,
) -> Result<TemplateRecord, YorishiroError> {
    crate::metaschema::validate_definition(&input.definition)?;
    let name = input.name.clone();
    let definition = serde_json::to_value(&input.definition).internal()?;

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(input.name),
        description: ActiveValue::Set(input.description),
        definition: ActiveValue::Set(definition),
        tags: ActiveValue::Set(input.tags),
        locale: ActiveValue::Set(input.locale),
        author: ActiveValue::Set(input.author),
        created_by: ActiveValue::Set(created_by),
        ..Default::default()
    };

    let row = active.insert(conn).await.map_err(|err| {
        if matches!(
            err.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            YorishiroError::Conflict {
                message: format!("a template named '{name}' already exists for this tenant"),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })?;

    row.try_into()
}

/// Updates a template's editable fields.
/// Only the owning tenant may update its own template (community-visible templates from other tenants are read-only to everyone but their owner).
pub async fn update_template(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    template_id: uuid::Uuid,
    input: UpdateTemplateInput,
) -> Result<TemplateRecord, YorishiroError> {
    if let Some(definition) = &input.definition {
        crate::metaschema::validate_definition(definition)?;
    }

    if input.name.is_none()
        && input.description.is_none()
        && input.definition.is_none()
        && input.tags.is_none()
        && input.locale.is_none()
    {
        return get_template(conn, tenant_id, template_id).await;
    }

    let existing = Entity::find()
        .filter(Column::Id.eq(template_id))
        .filter(Column::TenantId.eq(tenant_id))
        .one(conn)
        .await
        .internal()?
        .ok_or_else(|| {
            YorishiroError::not_found(format!("template '{template_id}' was not found"))
        })?;

    let mut active: ActiveModel = existing.into();
    if let Some(name) = input.name {
        active.name = ActiveValue::Set(name);
    }
    if let Some(description) = input.description {
        active.description = ActiveValue::Set(Some(description));
    }
    if let Some(definition) = input.definition {
        active.definition = ActiveValue::Set(serde_json::to_value(&definition).internal()?);
    }
    if let Some(tags) = input.tags {
        active.tags = ActiveValue::Set(tags);
    }
    if let Some(locale) = input.locale {
        active.locale = ActiveValue::Set(Some(locale));
    }

    active.update(conn).await.internal()?.try_into()
}

/// Deletes a template.
/// Only the owning tenant may delete it.
pub async fn delete_template(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    template_id: uuid::Uuid,
) -> Result<(), YorishiroError> {
    let result = Entity::delete_many()
        .filter(Column::Id.eq(template_id))
        .filter(Column::TenantId.eq(tenant_id))
        .exec(conn)
        .await
        .internal()?;

    if result.rows_affected == 0 {
        Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )))
    } else {
        Ok(())
    }
}

/// Copies a template (visible to `tenant_id`, i.e. own or community) into a new template owned by `tenant_id`, recording `fork_of` so the lineage is traceable.
pub async fn fork_template(
    conn: &impl ConnectionTrait,
    tenant_id: uuid::Uuid,
    created_by: Option<uuid::Uuid>,
    source_template_id: uuid::Uuid,
    new_name: String,
) -> Result<TemplateRecord, YorishiroError> {
    let source = get_template(conn, tenant_id, source_template_id).await?;
    let name = new_name.clone();
    let definition = serde_json::to_value(&source.definition).internal()?;

    let active = ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id),
        name: ActiveValue::Set(new_name),
        description: ActiveValue::Set(source.description),
        definition: ActiveValue::Set(definition),
        tags: ActiveValue::Set(source.tags),
        locale: ActiveValue::Set(source.locale),
        author: ActiveValue::Set(source.author),
        fork_of: ActiveValue::Set(Some(source.id)),
        created_by: ActiveValue::Set(created_by),
        ..Default::default()
    };

    let row = active.insert(conn).await.map_err(|err| {
        if matches!(
            err.sql_err(),
            Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
        ) {
            YorishiroError::Conflict {
                message: format!("a template named '{name}' already exists for this tenant"),
            }
        } else {
            YorishiroError::Internal(err.into())
        }
    })?;

    row.try_into()
}
