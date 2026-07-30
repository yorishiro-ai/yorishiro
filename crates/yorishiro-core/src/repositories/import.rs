use std::collections::HashMap;
use std::io::BufRead;

use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::repositories::entities::{self, CreateEntityInput};
use crate::repositories::relations::{self, CreateRelationInput};
use crate::repositories::schemas;

pub use crate::models::export::ExportRecord;
pub use crate::models::import::*;

/// Imports a JSON Lines document produced by `export::export_all` (or hand-written in the
/// same shape): one `{"kind":"schema"|"entity"|"relation","record":{...}}` object per line.
///
/// The whole read runs inside a single transaction, so it is all-or-nothing: the first
/// malformed line or repository-level failure (e.g. an entity referencing an unknown
/// schema, a relation referencing an unknown entity) rolls back everything imported so far
/// and returns `Err` describing the problem. On success, `ImportResult.errors` is always
/// empty.
///
/// Schemas and entities are re-inserted with freshly generated IDs (`create_schema`/
/// `entities::create` always mint a new one; entities are also re-validated against the
/// *current* active schema rather than trusting the exported `schema_version`), so:
///
/// - an entity line resolves its schema by *name*, preferring a schema line this same
///   import already processed over the (workspace-local, so likely meaningless here)
///   exported `schema_id`;
/// - a relation line's `source_id`/`target_id` are remapped through the entity lines this
///   same import already processed.
///
/// Because of this, a schema/entity line must appear before anything that references it --
/// exactly the order `export::export_all` produces (schemas, then entities, then relations).
pub async fn import_jsonl(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    reader: impl BufRead,
) -> Result<ImportResult, YorishiroError> {
    let mut tx = conn.begin().await.internal()?;

    let mut result = ImportResult::default();
    let mut entity_id_map: HashMap<Uuid, Uuid> = HashMap::new();
    // Exported `schema_id`s are only meaningful in the *source* workspace they came
    // from -- `create_schema` always mints a fresh ID. So entity lines can't resolve their
    // schema by re-querying the exported `schema_id` in the destination workspace; instead
    // track name-by-old-id for every schema line this import itself has processed so far.
    let mut schema_name_by_old_id: HashMap<Uuid, String> = HashMap::new();

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
                schemas::create_schema(&mut tx, workspace_id, schema.definition)
                    .await
                    .map_err(|err| annotate_line(line_no, err))?;
                schema_name_by_old_id.insert(old_id, name);
                result.schemas += 1;
            }
            ExportRecord::Entity(entity) => {
                let old_id = entity.id;

                // `entities::create` takes a schema *name* (it always resolves against the
                // workspace's currently active version), not a schema ID. Prefer the name
                // of a schema line this same import just created; a `schema_id` exported
                // from a different workspace means nothing here. Fall back to looking the
                // ID up in the destination workspace, for the case of importing entities
                // against a schema that already exists there (not part of this import).
                let schema_name = match schema_name_by_old_id.get(&entity.schema_id) {
                    Some(name) => name.clone(),
                    None => {
                        schemas::get_by_id(&mut tx, workspace_id, entity.schema_id)
                            .await
                            .map_err(|err| annotate_line(line_no, err))?
                            .name
                    }
                };

                let input = CreateEntityInput {
                    schema_name,
                    entity_type: entity.entity_type,
                    data: entity.data,
                };
                let created = entities::create(&mut tx, workspace_id, input, entity.created_by)
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
                relations::create(&mut tx, workspace_id, input)
                    .await
                    .map_err(|err| annotate_line(line_no, err))?;
                result.relations += 1;
            }
        }
    }

    tx.commit().await.internal()?;

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
