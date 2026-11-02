use std::io::{self, Write};

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::entity_entities::{self, EntityRecord};
use crate::models::entity_relations::{self, RelationRecord};
use crate::models::schema_schemas::{self, SchemaRecord};

/// One line of a JSONL export: a tagged union so schema/entity/relation records can be told apart on read-back without a separate line-position convention.
/// `Deserialize` is derived so `models::import::import_jsonl` can read the same shape back in.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub(crate) enum ExportRecord {
    Schema(SchemaRecord),
    Entity(EntityRecord),
    Relation(RelationRecord),
}

const MAX_EXPORT_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXPORT_RECORDS: usize = 10_000;
const PAGE_SIZE: u64 = 64;

/// Reads bounded pages and returns at most 8 MiB and 10,000 records, with schemas before their references.
pub(crate) async fn export_jsonl(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<String, YorishiroError> {
    use crate::models::_entities::{
        entity_entities as entities, entity_relations as relations, schema_schemas as schemas,
    };

    let mut output = ExportBuffer::default();
    let schemas = schema_schemas::Entity::find()
        .filter(schemas::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(schemas::Column::Id)
        .paginate(conn, PAGE_SIZE);
    let mut page = 0;
    loop {
        let rows = schemas.fetch_page(page).await.internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            output.append(&ExportRecord::Schema(row.try_into()?))?;
        }
        page += 1;
    }
    let entities = entity_entities::Entity::find()
        .filter(entities::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(entities::Column::Id)
        .into_model::<EntityRecord>()
        .paginate(conn, PAGE_SIZE);
    page = 0;
    loop {
        let rows = entities.fetch_page(page).await.internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            output.append(&ExportRecord::Entity(row))?;
        }
        page += 1;
    }
    let relations = entity_relations::Entity::find()
        .filter(relations::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(relations::Column::Id)
        .paginate(conn, PAGE_SIZE);
    page = 0;
    loop {
        let rows = relations.fetch_page(page).await.internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            output.append(&ExportRecord::Relation(row.try_into()?))?;
        }
        page += 1;
    }
    String::from_utf8(output.bytes).internal()
}

#[derive(Default)]
struct ExportBuffer {
    bytes: Vec<u8>,
    records: usize,
}

impl ExportBuffer {
    fn append(&mut self, record: &ExportRecord) -> Result<(), YorishiroError> {
        if self.records == MAX_EXPORT_RECORDS {
            return Err(export_too_large());
        }
        if let Err(error) = serde_json::to_writer(&mut *self, record) {
            return if error.is_io() {
                Err(export_too_large())
            } else {
                Err(error).internal()
            };
        }
        self.write_all(b"\n").map_err(|_| export_too_large())?;
        self.records += 1;
        Ok(())
    }
}

impl Write for ExportBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_EXPORT_BYTES.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("export byte limit reached"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn export_too_large() -> YorishiroError {
    YorishiroError::ValidationFailed {
        message: "workspace export exceeds the limit of 8 MiB or 10,000 records".into(),
        details: vec![],
        hint: "use the paginated schema, entity and relation list APIs for larger workspaces"
            .into(),
    }
}
