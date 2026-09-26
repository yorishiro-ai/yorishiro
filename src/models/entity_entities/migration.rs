use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, QuerySelect};
use serde_json::Value;
use uuid::Uuid;

use super::{
    ActiveModel, DriftField, DryRunByType, Entity, EntityDrift, EntityRecord, FillDefaultsReport,
    MigrationDryRun,
};
use crate::error::{ResultExt, YorishiroError};
use crate::metaschema;

/// Reports how `entity_id` stands against the active version of its schema.
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
    let own_fields = own
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);
    let active_fields = active
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);

    let missing_fields = match (own_fields, active_fields) {
        (Some(own_fields), Some(active_fields)) => active_fields
            .iter()
            .filter(|(name, _)| !own_fields.contains_key(*name))
            .map(|(name, def)| DriftField {
                name: name.clone(),
                r#type: def.r#type,
                required: def.required,
            })
            .collect(),
        _ => Vec::new(),
    };

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
/// The counting is done in one query per (entity_type, schema_id) group rather than one per entity: a workspace can hold far more entities than it holds distinct old versions, and the answer is the same either way.
pub async fn migration_dry_run(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    schema_name: &str,
) -> Result<MigrationDryRun, YorishiroError> {
    use crate::models::_entities::entity_entities::Column;

    let active =
        crate::models::schema_schemas::get_active_schema(conn, workspace_id, schema_name).await?;

    // (entity_type, schema_id, count) for everything under this schema name, whatever version.
    #[derive(sea_orm::FromQueryResult)]
    struct GroupedCount {
        entity_type: String,
        schema_id: Uuid,
        count: i64,
    }

    let rows: Vec<GroupedCount> = Entity::find()
        .select_only()
        .column(Column::EntityType)
        .column(Column::SchemaId)
        .column_as(Column::Id.count(), "count")
        .filter(Column::WorkspaceId.eq(workspace_id))
        .join(
            sea_orm::JoinType::InnerJoin,
            crate::models::_entities::entity_entities::Relation::SchemaSchemas.def(),
        )
        .filter(crate::models::_entities::schema_schemas::Column::Name.eq(schema_name))
        .group_by(Column::EntityType)
        .group_by(Column::SchemaId)
        .into_model::<GroupedCount>()
        .all(conn)
        .await
        .internal()?;

    let mut total = 0i64;
    let mut current = 0i64;
    let mut behind_valid = 0i64;
    let mut needs_values = 0i64;
    let mut by_type: std::collections::BTreeMap<String, DryRunByType> =
        std::collections::BTreeMap::new();

    // Each distinct old version is fetched once, not once per entity.
    let mut definitions: std::collections::HashMap<Uuid, metaschema::MetaSchemaDefinition> =
        std::collections::HashMap::new();

    for row in rows {
        total += row.count;

        if row.schema_id == active.id {
            current += row.count;
            continue;
        }

        let old = match definitions.get(&row.schema_id) {
            Some(def) => def.clone(),
            None => {
                let record =
                    crate::models::schema_schemas::get_by_id(conn, workspace_id, row.schema_id)
                        .await?;
                definitions.insert(row.schema_id, record.definition.clone());
                record.definition
            }
        };

        // Required in the active version, absent from the version these were written with.
        let missing: Vec<String> = match (
            active.definition.entity_types.get(&row.entity_type),
            old.entity_types.get(&row.entity_type),
        ) {
            (Some(active_type), Some(old_type)) => active_type
                .fields
                .iter()
                .filter(|(name, def)| def.required && !old_type.fields.contains_key(*name))
                .map(|(name, _)| name.clone())
                .collect(),
            _ => Vec::new(),
        };

        let entry = by_type
            .entry(row.entity_type.clone())
            .or_insert_with(|| DryRunByType {
                entity_type: row.entity_type,
                behind: 0,
                needs_values: 0,
                missing_required: Vec::new(),
            });
        entry.behind += row.count;

        if missing.is_empty() {
            behind_valid += row.count;
        } else {
            needs_values += row.count;
            entry.needs_values += row.count;
            for name in missing {
                if !entry.missing_required.contains(&name) {
                    entry.missing_required.push(name);
                }
            }
        }
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
pub async fn fill_defaults(
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
        #[allow(clippy::redundant_pattern_matching)]
        if let Err(_) = super::snapshots::snapshot(conn, workspace_id, record.id, job_id).await {
            continue; // Entity deleted since dry run; skip silently.
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
