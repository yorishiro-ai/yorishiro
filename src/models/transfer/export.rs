//! JSON Lines export of a workspace's schemas, entities and relations, bounded in size and record count.

use std::io::{self, Write};

use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::{entity_entities, entity_relations, schema_schemas};

use super::{EntityRecord, ExportRecord};

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
    let mut schema_cursor: Option<(String, i32)> = None;
    loop {
        let mut query = schema_schemas::Entity::find()
            .filter(schemas::Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(schemas::Column::Name)
            .order_by_asc(schemas::Column::Version)
            .limit(PAGE_SIZE);
        if let Some((name, version)) = &schema_cursor {
            query = query.filter(
                Condition::any()
                    .add(schemas::Column::Name.gt(name.clone()))
                    .add(
                        Condition::all()
                            .add(schemas::Column::Name.eq(name.clone()))
                            .add(schemas::Column::Version.gt(*version)),
                    ),
            );
        }
        let rows = query.all(conn).await.internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let cursor = (row.name.clone(), row.version);
            output.append(&ExportRecord::Schema(row.try_into()?))?;
            schema_cursor = Some(cursor);
        }
    }
    // Keyset pages do not shift when an earlier row is deleted between reads.
    let mut entity_cursor = None;
    loop {
        let mut query = entity_entities::Entity::find()
            .filter(entities::Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(entities::Column::Id)
            .limit(PAGE_SIZE);
        if let Some(id) = entity_cursor {
            query = query.filter(entities::Column::Id.gt(id));
        }
        let rows = query
            .into_model::<EntityRecord>()
            .all(conn)
            .await
            .internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let id = row.id;
            output.append(&ExportRecord::Entity(row))?;
            entity_cursor = Some(id);
        }
    }
    let mut relation_cursor = None;
    loop {
        let mut query = entity_relations::Entity::find()
            .filter(relations::Column::WorkspaceId.eq(workspace_id))
            .order_by_asc(relations::Column::Id)
            .limit(PAGE_SIZE);
        if let Some(id) = relation_cursor {
            query = query.filter(relations::Column::Id.gt(id));
        }
        let rows = query.all(conn).await.internal()?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let id = row.id;
            output.append(&ExportRecord::Relation(row.try_into()?))?;
            relation_cursor = Some(id);
        }
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

/// Opens the caller's read session and exports the workspace as JSON Lines.
///
/// # Errors
/// Returns the session, authorization or export error.
pub(crate) async fn export_jsonl_for_actor(
    ctx: &loco_rs::app::AppContext,
    actor: crate::models::api_keys::AuthContext,
) -> Result<String, YorishiroError> {
    crate::models::iam::session::WorkspaceSession::read(
        ctx,
        actor,
        crate::models::api_keys::ApiKeyScope::Read,
        crate::models::iam::access::WorkspaceTarget::Permission(
            crate::models::iam::policy::Action::Export,
        ),
        crate::models::iam::policy::Action::Export,
        async |session| export_jsonl(session.txn(), session.workspace_id()?).await,
    )
    .await
}
