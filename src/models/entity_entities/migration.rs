use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, PaginatorTrait, QueryOrder, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

use super::{
    ActiveModel, DriftField, DryRunByType, Entity, EntityDrift, EntityRecord, FillDefaultsReport,
    MigrationDryRun,
};
use crate::error::{ResultExt, YorishiroError};
use crate::models::schema_schemas::metaschema;

/// Reports how `entity_id` stands against the active version of its schema.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn drift(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
) -> Result<EntityDrift, YorishiroError> {
    let entity = super::crud::get(conn, workspace_id, entity_id).await?;
    let own =
        crate::models::schema_schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
    let active =
        crate::models::schema_schemas::get_active_schema(conn, workspace_id, &own.name).await?;

    // The entity's own type definition may be absent from the active version: the type was dropped.
    // Nothing is "missing" in that case; the whole type is, which the version numbers already say.
    let active_fields = active
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);

    let missing_fields = active_fields
        .map(|fields| missing_fields_in_value(&entity.data, fields, ""))
        .unwrap_or_default();

    Ok(EntityDrift {
        entity_id: entity.id,
        entity_type: entity.entity_type,
        schema_version: entity.schema_version,
        active_schema_version: active.version,
        missing_fields,
    })
}

/// Counts what a batch migration to `schema_name`'s active version would face.
/// Reads only.
///
/// Reads stored data in bounded pages because entities on the same schema version can have different missing values.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn migration_dry_run(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<MigrationDryRun, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    let active =
        crate::models::schema_schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    let mut rows = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            crate::models::_entities::entity_entities::Relation::SchemaSchemas.def(),
        )
        .filter(crate::models::_entities::schema_schemas::Column::Name.eq(schema_name))
        .order_by_asc(Column::Id)
        .paginate(conn, crate::models::pagination::MAX_LIST_LIMIT as u64);

    let mut total = 0i64;
    let mut current = 0i64;
    let mut behind_valid = 0i64;
    let mut needs_values = 0i64;
    let mut by_type: std::collections::BTreeMap<String, DryRunByType> =
        std::collections::BTreeMap::new();

    while let Some(page) = rows.fetch_and_next().await.internal()? {
        for row in page {
            total += 1;
            if row.schema_id == active.id {
                current += 1;
                continue;
            }
            let missing: Vec<String> = active
                .definition
                .entity_types
                .get(&row.entity_type)
                .map(|definition| missing_fields_in_value(&row.data, &definition.fields, ""))
                .unwrap_or_default()
                .into_iter()
                .filter(|field| field.required)
                .map(|field| field.name)
                .collect();

            let entry = by_type
                .entry(row.entity_type.clone())
                .or_insert_with(|| DryRunByType {
                    entity_type: row.entity_type,
                    behind: 0,
                    needs_values: 0,
                    missing_required: Vec::new(),
                });
            entry.behind += 1;

            if missing.is_empty() {
                behind_valid += 1;
            } else {
                needs_values += 1;
                entry.needs_values += 1;
                for name in missing {
                    if !entry.missing_required.contains(&name) {
                        entry.missing_required.push(name);
                    }
                }
            }
        }
    }

    for entry in by_type.values_mut() {
        entry.missing_required.sort();
    }

    Ok(MigrationDryRun {
        schema_name: schema_name.to_string(),
        active_version: active.version,
        total_entities: total,
        current,
        behind_but_valid: behind_valid,
        needs_values,
        by_entity_type: by_type.into_values().collect(),
    })
}

/// Lists absent fields, descending only into existing objects as fill-defaults does.
/// A present null or a value of the wrong type is not an absent field.
fn missing_fields_in_value(
    data: &Value,
    fields: &std::collections::BTreeMap<String, metaschema::FieldDef>,
    parent: &str,
) -> Vec<DriftField> {
    let Some(object) = data.as_object() else {
        return Vec::new();
    };
    let mut missing = Vec::new();
    for (name, definition) in fields {
        let pointer = format!("{parent}/{}", name.replace('~', "~0").replace('/', "~1"));
        match object.get(name) {
            None => missing.push(DriftField {
                name: if parent.is_empty() {
                    name.clone()
                } else {
                    pointer.clone()
                },
                r#type: definition.r#type,
                required: definition.required,
            }),
            Some(value) => {
                if let Some(properties) = &definition.properties {
                    missing.extend(missing_fields_in_value(value, properties, &pointer));
                }
            }
        }
    }
    missing
}

/// Fills absent required fields in entities that fall behind a schema version.
///
/// For each entity behind the active version:
/// 1. Takes a snapshot (so the batch can be undone via `undo_job`).
/// 2. Reads the entity's current data and the active version's field definitions.
/// 3. For every required field the active version defines but the entity lacks, fills it with the
///    field's `default` value, or a type-appropriate empty value if no default is set.
/// 4. Writes the updated data back.
///
/// Returns a report counting entities updated and total fields filled.
///
/// `updated_by` is the acting user's ID, or `None` for an unattributed API key.
pub(crate) async fn fill_defaults(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_name: &str,
    job_id: Uuid,
    updated_by: Option<Uuid>,
) -> Result<FillDefaultsReport, YorishiroError> {
    let active =
        crate::models::schema_schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    // Fetch all entities behind the active version.
    use crate::models::_entities::entity_entities::Column;

    let rows: Vec<EntityRecord> = Entity::find()
        .filter(Column::WorkspaceId.eq(workspace_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            crate::models::_entities::entity_entities::Relation::SchemaSchemas.def(),
        )
        .filter(crate::models::_entities::schema_schemas::Column::Name.eq(schema_name))
        .filter(Column::SchemaId.ne(active.id))
        .into_model::<EntityRecord>()
        .all(conn)
        .await
        .internal()?;

    if rows.is_empty() {
        return Ok(FillDefaultsReport {
            schema_name: schema_name.to_string(),
            job_id,
            entities_updated: 0,
            fields_filled: 0,
        });
    }

    let mut entities_updated = 0i64;
    let mut fields_filled = 0i64;

    for record in rows {
        let entity_type = &record.entity_type;

        // Find the active version's entity type definition.
        let active_entity = match active.definition.entity_types.get(entity_type.as_str()) {
            Some(def) => def,
            None => continue, // Entity type dropped in active version; nothing to fill.
        };

        // Take a snapshot before modifying.
        if let Err(error) = super::snapshots::snapshot(conn, workspace_id, record.id, job_id).await
        {
            if matches!(error, YorishiroError::NotFound { .. }) {
                continue;
            }
            return Err(error);
        }

        let mut data = record.data.clone();
        let filled = fill_fields_in_value(&mut data, &active_entity.fields);
        if filled > 0 {
            let active = ActiveModel {
                id: ActiveValue::Unchanged(record.id),
                data: ActiveValue::Set(data),
                updated_by: ActiveValue::Set(updated_by),
                ..Default::default()
            };
            match active.update_without_returning(conn).await {
                Ok(_) => {
                    entities_updated += 1;
                    fields_filled += filled;
                }
                Err(DbErr::RecordNotUpdated) => {}
                Err(err) => return Err(err).internal(),
            }
        }
    }

    Ok(FillDefaultsReport {
        schema_name: schema_name.to_string(),
        job_id,
        entities_updated,
        fields_filled,
    })
}

/// Recursively fill missing required fields in a JSON value.
///
/// Returns the count of fields filled (including nested objects).
fn fill_fields_in_value(
    data: &mut Value,
    fields: &std::collections::BTreeMap<String, metaschema::FieldDef>,
) -> i64 {
    let mut count = 0i64;

    if !data.is_object() {
        return count;
    }

    let obj = data.as_object_mut().unwrap();

    for (name, prop_def) in fields {
        if obj.contains_key(name) {
            // Field exists; recurse into nested objects.
            if let Some(child) = obj.get_mut(name)
                && let Some(ref_properties) = &prop_def.properties
            {
                count += fill_fields_in_value(child, ref_properties);
            }
        } else if prop_def.required {
            // Missing required field — fill it.
            if let Some(ref_val) = &prop_def.default {
                obj.insert(name.clone(), ref_val.clone());
            } else {
                let empty = empty_value_for_type(&prop_def.r#type);
                obj.insert(name.clone(), empty);
            }
            count += 1;
        }
    }

    count
}

/// Returns a type-appropriate empty value for a field type.
fn empty_value_for_type(type_name: &metaschema::FieldTypeName) -> Value {
    match type_name {
        metaschema::FieldTypeName::Boolean => Value::Bool(false),
        metaschema::FieldTypeName::Integer | metaschema::FieldTypeName::Number => {
            Value::Number(serde_json::Number::from(0))
        }
        metaschema::FieldTypeName::String => Value::String(String::new()),
        metaschema::FieldTypeName::Object => Value::Object(serde_json::Map::new()),
        metaschema::FieldTypeName::Array => Value::Array(vec![]),
    }
}
