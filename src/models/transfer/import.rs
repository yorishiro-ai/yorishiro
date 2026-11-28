//! JSON Lines import that reads what `export` writes, re-creating schemas, entities and relations in the caller's tenant.

use std::collections::HashMap;
use std::io::BufRead;

use sea_orm::ConnectionTrait;
use serde::Serialize;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::{self, CreateEntityInput};
use crate::models::entity_relations::{self, CreateRelationInput};
use crate::models::schema_schemas;

use super::ExportRecord;

/// Outcome of a successful import: how many records of each kind were inserted.
//
// The caller supplies an RLS-scoped transaction and commits only after all records and embedding jobs succeed.
#[derive(Clone, Default, Serialize, schemars::JsonSchema)]
pub struct ImportResult {
    pub schemas: u64,
    pub entities: u64,
    pub relations: u64,
    #[serde(skip)]
    pub(crate) entity_ids: Vec<Uuid>,
}

/// Imports a JSON Lines document produced by `export::export_jsonl` (or hand-written in the same shape): one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
///
/// Records are re-inserted with freshly generated IDs.
/// An entity line resolves its schema by name, preferring a schema line this import already processed over the exported `schema_id`.
/// A relation line's `source_id` and `target_id` are remapped through the entity lines already processed.
/// A schema or entity line must therefore appear before anything that references it, which is the order `export::export_jsonl` produces.
///
/// `tenant_id` comes from the authenticated request, not the export, because an exported `tenant_id` is only meaningful in its source tenant.
///
/// Every imported entity is attributed to `imported_by`, not the exported `created_by`, which names a user the destination tenant may not have.
///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn import_jsonl(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    imported_by: Option<Uuid>,
    reader: impl BufRead,
) -> Result<ImportResult, YorishiroError> {
    import_with_quota(conn, tenant_id, workspace_id, imported_by, reader, None).await
}

pub(crate) async fn import_with_quota(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    imported_by: Option<Uuid>,
    reader: impl BufRead,
    entity_quota: Option<i32>,
) -> Result<ImportResult, YorishiroError> {
    let mut result = ImportResult::default();
    let mut entity_id_map: HashMap<Uuid, Uuid> = HashMap::new();
    // Exported schema ids belong to the source workspace, so map the old id to the name of each schema line processed here.
    let mut schema_name_by_old_id: HashMap<Uuid, String> = HashMap::new();
    // Memoizes lookups of schemas that already existed in the destination workspace.
    let mut schema_name_by_id: HashMap<Uuid, String> = HashMap::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line_no = line_no + 1;
        let line = line.internal()?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let record: ExportRecord =
            serde_json::from_str(line).map_err(|err| YorishiroError::ValidationFailed {
                message: format!("line {line_no}: invalid JSONL record: {err}"),
                details: vec![],
                hint: "each line must be a JSON object of the form \
                       {\"kind\":\"schema\"|\"entity\"|\"relation\",\"record\":{...}}"
                    .into(),
            })?;

        match record {
            ExportRecord::Schema(schema) => {
                let old_id = schema.id;
                let name = schema.definition.name.clone();
                schema_schemas::create_schema(
                    conn,
                    tenant_id,
                    workspace_id,
                    schema.definition,
                    None,
                    None,
                )
                .await
                .map_err(|err| annotate_line(line_no, err))?;
                schema_name_by_old_id.insert(old_id, name);
                result.schemas += 1;
            }
            ExportRecord::Entity(entity) => {
                let old_id = entity.id;

                // `create` takes a schema name: use the schema line from this import, else look the id up in the destination workspace.
                let schema_name = match schema_name_by_old_id.get(&entity.schema_id) {
                    Some(name) => name.clone(),
                    None => {
                        if let Some(name) = schema_name_by_id.get(&entity.schema_id) {
                            name.clone()
                        } else {
                            let name =
                                schema_schemas::get_by_id(conn, workspace_id, entity.schema_id)
                                    .await
                                    .map_err(|err| annotate_line(line_no, err))?
                                    .name;
                            schema_name_by_id.insert(entity.schema_id, name.clone());
                            name
                        }
                    }
                };

                let input = CreateEntityInput {
                    schema_name,
                    entity_type: entity.entity_type,
                    data: entity.data,
                };
                let created = entity_entities::create_with_quota(
                    conn,
                    workspace_id,
                    input,
                    imported_by,
                    entity_quota,
                )
                .await
                .map_err(|err| annotate_line(line_no, err))?;
                entity_id_map.insert(old_id, created.id);
                result.entity_ids.push(created.id);
                result.entities += 1;
            }
            ExportRecord::Relation(relation) => {
                let source_id = entity_id_map
                    .get(&relation.source_id)
                    .copied()
                    .unwrap_or(relation.source_id);
                let target_id = entity_id_map
                    .get(&relation.target_id)
                    .copied()
                    .unwrap_or(relation.target_id);

                let input = CreateRelationInput {
                    source_id,
                    target_id,
                    relation_type: relation.relation_type,
                    properties: relation.properties,
                };
                entity_relations::create(conn, workspace_id, input)
                    .await
                    .map_err(|err| annotate_line(line_no, err))?;
                result.relations += 1;
            }
        }
    }

    Ok(result)
}

fn annotate_line(line_no: usize, err: YorishiroError) -> YorishiroError {
    match err {
        YorishiroError::ValidationFailed {
            message,
            details,
            hint,
        } => YorishiroError::ValidationFailed {
            message: format!("line {line_no}: {message}"),
            details,
            hint,
        },
        YorishiroError::NotFound { message } => YorishiroError::NotFound {
            message: format!("line {line_no}: {message}"),
        },
        YorishiroError::Conflict { message } => YorishiroError::Conflict {
            message: format!("line {line_no}: {message}"),
        },
        YorishiroError::RelationTypeMismatch { message } => YorishiroError::RelationTypeMismatch {
            message: format!("line {line_no}: {message}"),
        },
        other => other,
    }
}
