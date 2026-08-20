//! CRUD for `identity.templates`, the user-contributed schema template library.
//! Distinct from `crate::templates` (the built-in templates shipped with the binary and served from memory):
//! these are tenant-scoped, DB-backed templates that a tenant's members create and manage through `/api/template-library`.
//! Operates on `&PgPool` (the identity pool) rather than an RLS-scoped connection, matching the rest of this module: `identity.templates` has no RLS of its own, so every function here takes a `tenant_id` and filters/checks ownership explicitly.

use chrono::{DateTime, Utc};
use sea_query::{Alias, Expr, Iden, Order, PostgresQueryBuilder, Query};
use sea_query_binder::SqlxBinder;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::metaschema::{MetaSchemaDefinition, validate_definition};
use crate::models::tenancy::{CreateTemplateInput, TemplateRecord, UpdateTemplateInput};

#[derive(Iden)]
enum Templates {
    Table,
    Id,
    TenantId,
    Name,
    Description,
    Definition,
    Tags,
    Locale,
    Visibility,
    Author,
    ForkOf,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(sqlx::FromRow)]
struct TemplateRow {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    description: Option<String>,
    definition: serde_json::Value,
    tags: Vec<String>,
    locale: Option<String>,
    visibility: String,
    author: Option<String>,
    fork_of: Option<Uuid>,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TemplateRow {
    fn into_record(self) -> Result<TemplateRecord, YorishiroError> {
        let definition = serde_json::from_value(self.definition).internal()?;
        Ok(TemplateRecord {
            id: self.id,
            tenant_id: self.tenant_id,
            name: self.name,
            description: self.description,
            definition,
            tags: self.tags,
            locale: self.locale,
            visibility: self.visibility,
            author: self.author,
            fork_of: self.fork_of,
            created_by: self.created_by,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

fn template_columns() -> [Templates; 13] {
    [
        Templates::Id,
        Templates::TenantId,
        Templates::Name,
        Templates::Description,
        Templates::Definition,
        Templates::Tags,
        Templates::Locale,
        Templates::Visibility,
        Templates::Author,
        Templates::ForkOf,
        Templates::CreatedBy,
        Templates::CreatedAt,
        Templates::UpdatedAt,
    ]
}

/// Lists templates visible to `tenant_id`: its own templates plus any published with community visibility (cross-tenant sharing; not yet reachable through the API, but the query already honors it so nothing else needs to change when publishing ships).
pub async fn list_templates(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<TemplateRecord>, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(template_columns())
        .from((Alias::new("identity"), Templates::Table))
        .and_where(
            Expr::col(Templates::TenantId)
                .eq(tenant_id)
                .or(Expr::col(Templates::Visibility).eq("community")),
        )
        .order_by(Templates::CreatedAt, Order::Asc)
        .build_sqlx(PostgresQueryBuilder);

    let rows: Vec<TemplateRow> = sqlx::query_as_with(&sql, values)
        .fetch_all(pool)
        .await
        .internal()?;

    rows.into_iter().map(TemplateRow::into_record).collect()
}

/// Fetches a single template, allowed when it belongs to `tenant_id` or is community-visible.
pub async fn get_template(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<TemplateRecord, YorishiroError> {
    let (sql, values) = Query::select()
        .columns(template_columns())
        .from((Alias::new("identity"), Templates::Table))
        .and_where(Expr::col(Templates::Id).eq(template_id))
        .and_where(
            Expr::col(Templates::TenantId)
                .eq(tenant_id)
                .or(Expr::col(Templates::Visibility).eq("community")),
        )
        .build_sqlx(PostgresQueryBuilder);

    let row: Option<TemplateRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    row.ok_or_else(|| YorishiroError::not_found(format!("template '{template_id}' was not found")))?
        .into_record()
}

/// Resolves a `template_id` as either a library template or a built-in, and says which.
///
/// A UUID can only mean the library; anything else can only mean a built-in.
/// Parsing decides which, so neither lookup runs against an id that could not name it, and a library miss reports the library's own not-found rather than the built-in one.
///
/// The returned id is the origin to record: `Some` for a library template, whose later edits the schema can then be told about, and `None` for a built-in, which has no row to point at.
/// Shared by both adapters, because the same id resolved differently by REST and MCP would leave one of them holding a schema that silently forgot where it came from.
pub async fn resolve_template_definition(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: &str,
) -> Result<(MetaSchemaDefinition, Option<Uuid>), YorishiroError> {
    match Uuid::parse_str(template_id) {
        Ok(id) => {
            let template = get_template(pool, tenant_id, id).await?;
            Ok((template.definition, Some(template.id)))
        }
        Err(_) => Ok((crate::templates::get_template(template_id)?, None)),
    }
}

/// Creates a template owned by `tenant_id`.
/// Every new template starts as tenant-private (`visibility = 'tenant'`); there is no public creation path for `community` visibility yet.
pub async fn create_template(
    pool: &PgPool,
    tenant_id: Uuid,
    created_by: Option<Uuid>,
    input: CreateTemplateInput,
) -> Result<TemplateRecord, YorishiroError> {
    validate_definition(&input.definition)?;
    let definition = serde_json::to_value(&input.definition).internal()?;
    let name = input.name.clone();

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Templates::Table))
        .columns([
            Templates::TenantId,
            Templates::Name,
            Templates::Description,
            Templates::Definition,
            Templates::Tags,
            Templates::Locale,
            Templates::Author,
            Templates::CreatedBy,
        ])
        .values_panic([
            tenant_id.into(),
            input.name.into(),
            input.description.into(),
            definition.into(),
            input.tags.into(),
            input.locale.into(),
            input.author.into(),
            created_by.into(),
        ])
        .returning(Query::returning().columns(template_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: TemplateRow = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .map_err(|err| {
            if let sqlx::Error::Database(db_err) = &err
                && db_err.is_unique_violation()
            {
                YorishiroError::Conflict {
                    message: format!("a template named '{name}' already exists for this tenant"),
                }
            } else {
                YorishiroError::Internal(err.into())
            }
        })?;

    row.into_record()
}

/// Updates a template's editable fields.
/// Only the owning tenant may update its own template (community-visible templates from other tenants are read-only to everyone but their owner).
pub async fn update_template(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
    input: UpdateTemplateInput,
) -> Result<TemplateRecord, YorishiroError> {
    if let Some(definition) = &input.definition {
        validate_definition(definition)?;
    }

    let mut update = Query::update();
    update.table((Alias::new("identity"), Templates::Table));

    let mut values: Vec<(Templates, sea_query::SimpleExpr)> = vec![];
    if let Some(name) = input.name {
        values.push((Templates::Name, name.into()));
    }
    if let Some(description) = input.description {
        values.push((Templates::Description, Some(description).into()));
    }
    if let Some(definition) = input.definition {
        let definition = serde_json::to_value(&definition).internal()?;
        values.push((Templates::Definition, definition.into()));
    }
    if let Some(tags) = input.tags {
        values.push((Templates::Tags, tags.into()));
    }
    if let Some(locale) = input.locale {
        values.push((Templates::Locale, Some(locale).into()));
    }

    if values.is_empty() {
        return get_template(pool, tenant_id, template_id).await;
    }

    update.values(values);
    update.and_where(Expr::col(Templates::Id).eq(template_id));
    update.and_where(Expr::col(Templates::TenantId).eq(tenant_id));
    update.returning(Query::returning().columns(template_columns()));
    let (sql, values) = update.build_sqlx(PostgresQueryBuilder);

    let row: Option<TemplateRow> = sqlx::query_as_with(&sql, values)
        .fetch_optional(pool)
        .await
        .internal()?;

    row.ok_or_else(|| YorishiroError::not_found(format!("template '{template_id}' was not found")))?
        .into_record()
}

/// Deletes a template.
/// Only the owning tenant may delete it.
pub async fn delete_template(
    pool: &PgPool,
    tenant_id: Uuid,
    template_id: Uuid,
) -> Result<(), YorishiroError> {
    let (sql, values) = Query::delete()
        .from_table((Alias::new("identity"), Templates::Table))
        .and_where(Expr::col(Templates::Id).eq(template_id))
        .and_where(Expr::col(Templates::TenantId).eq(tenant_id))
        .build_sqlx(PostgresQueryBuilder);

    let result = sqlx::query_with(&sql, values)
        .execute(pool)
        .await
        .internal()?;

    if result.rows_affected() == 0 {
        Err(YorishiroError::not_found(format!(
            "template '{template_id}' was not found"
        )))
    } else {
        Ok(())
    }
}

/// Copies a template (visible to `tenant_id`, i.e. own or community) into a new template owned by `tenant_id`, recording `fork_of` so the lineage is traceable.
pub async fn fork_template(
    pool: &PgPool,
    tenant_id: Uuid,
    created_by: Option<Uuid>,
    source_template_id: Uuid,
    new_name: String,
) -> Result<TemplateRecord, YorishiroError> {
    let source = get_template(pool, tenant_id, source_template_id).await?;
    let definition = serde_json::to_value(&source.definition).internal()?;
    let name = new_name.clone();

    let (sql, values) = Query::insert()
        .into_table((Alias::new("identity"), Templates::Table))
        .columns([
            Templates::TenantId,
            Templates::Name,
            Templates::Description,
            Templates::Definition,
            Templates::Tags,
            Templates::Locale,
            Templates::Author,
            Templates::ForkOf,
            Templates::CreatedBy,
        ])
        .values_panic([
            tenant_id.into(),
            new_name.into(),
            source.description.into(),
            definition.into(),
            source.tags.into(),
            source.locale.into(),
            source.author.into(),
            source.id.into(),
            created_by.into(),
        ])
        .returning(Query::returning().columns(template_columns()))
        .build_sqlx(PostgresQueryBuilder);

    let row: TemplateRow = sqlx::query_as_with(&sql, values)
        .fetch_one(pool)
        .await
        .map_err(|err| {
            if let sqlx::Error::Database(db_err) = &err
                && db_err.is_unique_violation()
            {
                YorishiroError::Conflict {
                    message: format!("a template named '{name}' already exists for this tenant"),
                }
            } else {
                YorishiroError::Internal(err.into())
            }
        })?;

    row.into_record()
}

#[cfg(test)]
#[path = "../../../../tests/models/tenancy/template_library/mod.rs"]
mod tests;
