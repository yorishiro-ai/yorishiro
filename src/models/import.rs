use std::collections::HashMap;
use std::io::BufRead;

use sea_orm::ConnectionTrait;
use serde::Serialize;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::content_entities::{self, CreateEntityInput};
use crate::models::content_relations::{self, CreateRelationInput};
use crate::models::content_schemas;

pub use crate::models::export::ExportRecord;

/// Outcome of a successful `import_jsonl` call: how many records of each kind were inserted.
/// The whole import runs on the caller's RLS-scoped request transaction, so an error return means the caller never calls `Authorized::commit()` and nothing is applied (rollback on drop).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportResult {
    pub schemas: u64,
    pub entities: u64,
    pub relations: u64,
}

/// Imports a JSON Lines document produced by `export::export_all` (or hand-written in the same shape): one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
///
/// Schemas and entities are re-inserted with freshly generated IDs, so an entity line resolves its schema by *name* (preferring a schema line this same import already processed over the exported, workspace-local `schema_id`), and a relation line's `source_id`/`target_id` are remapped through the entity lines this same import already processed.
/// A schema/entity line must therefore appear before anything that references it: the order `export::export_all` produces (schemas, then entities, then relations).
///
/// `tenant_id` comes from the authenticated request, not the export: an exported schema's `tenant_id` is only meaningful in its source tenant, and reusing it here would violate `content_schemas`' FK to `identity_tenants` once source and destination tenants differ.
///
/// Every imported entity is attributed to `imported_by`, not the exported `created_by`: the exported value names a user in the source tenant that `yorishiro_app` can't verify exists (`identity_users` carries no grant to that role), risking an FK violation on restore.
pub async fn import_jsonl(
    conn: &impl ConnectionTrait,
    tenant_id: Uuid,
    workspace_id: Uuid,
    imported_by: Option<Uuid>,
    reader: impl BufRead,
) -> Result<ImportResult, YorishiroError> {
    let mut result = ImportResult::default();
    let mut entity_id_map: HashMap<Uuid, Uuid> = HashMap::new();
    // Exported schema_ids are workspace-local to the source, so track name-by-old-id for schema lines this import has processed instead of re-querying the exported id.
    let mut schema_name_by_old_id: HashMap<Uuid, String> = HashMap::new();
    // A schema that already existed in the destination workspace before this import (not itself a schema line in this file) falls through to get_by_id below on every entity line that references it; memoized the same way migration_dry_run and recall_context cache a schema lookup, since a batch of entities sharing one pre-existing schema is the common case, not the exception.
    let mut schema_name_by_id: HashMap<Uuid, String> = HashMap::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line_no = line_no + 1;
        let line = line.map_err(|err| YorishiroError::Internal(err.into()))?;
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
                content_schemas::create_schema(
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

                // `content_entities::create` takes a schema name, not an ID.
                // Prefer a schema line this import just created; fall back to looking the exported ID up in the destination workspace, for a schema that already exists there.
                let schema_name = match schema_name_by_old_id.get(&entity.schema_id) {
                    Some(name) => name.clone(),
                    None => match schema_name_by_id.get(&entity.schema_id) {
                        Some(name) => name.clone(),
                        None => {
                            let name =
                                content_schemas::get_by_id(conn, workspace_id, entity.schema_id)
                                    .await
                                    .map_err(|err| annotate_line(line_no, err))?
                                    .name;
                            schema_name_by_id.insert(entity.schema_id, name.clone());
                            name
                        }
                    },
                };

                let input = CreateEntityInput {
                    schema_name,
                    entity_type: entity.entity_type,
                    data: entity.data,
                };
                let created = content_entities::create(conn, workspace_id, input, imported_by)
                    .await
                    .map_err(|err| annotate_line(line_no, err))?;
                entity_id_map.insert(old_id, created.id);
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
                content_relations::create(conn, workspace_id, input)
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
