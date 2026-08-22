//! Fetches an entity's full body together with its relations and connected neighbors, up to
//! `depth` hops away, in one call. Ported from master's `models::recall`.

use std::collections::{HashMap, HashSet};

use sea_orm::ConnectionTrait;
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::content_entities::{self, EntityRecord};
use crate::models::content_relations::{self, DEFAULT_NEIGHBORS_LIMIT};
use crate::models::content_schemas::{self, SchemaRecord};

pub const DEFAULT_RECALL_LIMIT: i64 = DEFAULT_NEIGHBORS_LIMIT;

/// Default number of hops `recall_context` traverses when `depth` is omitted.
pub const DEFAULT_RECALL_DEPTH: i64 = 1;

/// Upper bound on `depth`, clamped in `recall_context`, to prevent runaway fan-out queries on
/// dense graphs.
pub const MAX_RECALL_DEPTH: i64 = 3;

#[derive(Debug, Clone, Serialize)]
pub struct RecallRelation {
    pub relation_type: String,
    pub direction: String,
    /// The connected entity. Shallow (only `x-embed` fields in `data`) by default; pass
    /// `full: true` to `recall_context` to get every field instead.
    pub neighbor: EntityRecord,
    /// How many hops away from the requested entity this neighbor is (1 = direct neighbor, 2 =
    /// neighbor-of-neighbor, etc).
    pub hop_distance: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallContext {
    /// The requested entity, always with its full `data`.
    pub entity: EntityRecord,
    /// Flat list of every neighbor found within `depth` hops, each tagged with its
    /// `hop_distance`. A neighbor reachable via more than one path is only reported once, at the
    /// shortest hop_distance it was found at.
    pub relations: Vec<RecallRelation>,
    /// `true` when more neighbors exist at some hop than `limit` allowed to be included there.
    pub truncated: bool,
}

/// Parameters for [`recall_context`].
#[derive(Debug, Clone, Copy)]
pub struct RecallQuery {
    /// Maximum number of relations/neighbors to fetch per hop.
    pub limit: i64,
    /// When true, neighbor entities include every field instead of only `x-embed` fields.
    pub full: bool,
    /// How many hops to traverse outward from the requested entity. Clamped to
    /// `[1, MAX_RECALL_DEPTH]`.
    pub depth: i64,
}

impl Default for RecallQuery {
    fn default() -> Self {
        Self {
            limit: DEFAULT_RECALL_LIMIT,
            full: false,
            depth: DEFAULT_RECALL_DEPTH,
        }
    }
}

/// Reduces `entity.data` down to only the fields marked `x-embed` in its entity_type definition.
/// Falls back to an empty body if the entity's schema version no longer defines that entity_type
/// at all, rather than failing the whole recall for one neighbor.
fn shallow_copy(schema: &SchemaRecord, mut entity: EntityRecord) -> EntityRecord {
    let fields = schema
        .definition
        .entity_types
        .get(&entity.entity_type)
        .map(|def| &def.fields);

    let mut shallow = Map::new();
    if let (Some(fields), Value::Object(data)) = (fields, &entity.data) {
        for (name, field_def) in fields {
            if field_def.x_embed
                && let Some(value) = data.get(name)
            {
                shallow.insert(name.clone(), value.clone());
            }
        }
    }
    entity.data = Value::Object(shallow);
    entity
}

/// Fetches an entity's full body together with its relations and connected neighbors in one
/// call, so a caller doesn't need `entity_get` + `list_relations` + `entity_get` per neighbor
/// round trips.
///
/// `query.depth` controls how many hops are traversed outward from `entity_id`. At depth 1 this
/// is exactly the original single-hop behavior. At depth > 1, each subsequent hop fetches the
/// neighbors of every entity discovered at the previous hop, breadth-first. An entity reachable
/// by more than one path is only reported once, tagged with the shortest `hop_distance`, and
/// never re-expanded once visited (so cycles terminate).
///
/// Runs on the RLS-scoped transaction a request handler holds via `Authorized::txn()`.
pub async fn recall_context(
    conn: &impl ConnectionTrait,
    workspace_id: Uuid,
    entity_id: Uuid,
    query: RecallQuery,
) -> Result<RecallContext, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let depth = query.depth.clamp(1, MAX_RECALL_DEPTH);
    let full = query.full;

    let entity = content_entities::get(conn, workspace_id, entity_id).await?;

    let mut schema_cache: HashMap<Uuid, SchemaRecord> = HashMap::new();

    let mut visited: HashSet<Uuid> = HashSet::from([entity_id]);
    let mut frontier: Vec<Uuid> = vec![entity_id];
    let mut relations_out = Vec::new();
    let mut truncated = false;

    for hop in 1..=depth {
        let mut next_frontier = Vec::new();

        let mut by_pivot =
            content_relations::neighbors_batch(conn, workspace_id, &frontier, limit + 1).await?;

        for &source_id in &frontier {
            let Some(mut node_neighbors) = by_pivot.remove(&source_id) else {
                continue;
            };
            if node_neighbors.len() as i64 > limit {
                truncated = true;
            }
            node_neighbors.truncate(limit as usize);

            for neighbor in node_neighbors {
                if !visited.insert(neighbor.entity.id) {
                    continue;
                }
                next_frontier.push(neighbor.entity.id);

                let neighbor_entity = if full {
                    neighbor.entity
                } else {
                    let schema_id = neighbor.entity.schema_id;
                    let schema = match schema_cache.get(&schema_id) {
                        Some(schema) => schema,
                        None => {
                            let schema =
                                content_schemas::get_by_id(conn, workspace_id, schema_id).await?;
                            schema_cache.entry(schema_id).or_insert(schema)
                        }
                    };
                    shallow_copy(schema, neighbor.entity)
                };
                relations_out.push(RecallRelation {
                    relation_type: neighbor.relation_type,
                    direction: neighbor.direction,
                    neighbor: neighbor_entity,
                    hop_distance: hop as i32,
                });
            }
        }

        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
    }

    Ok(RecallContext {
        entity,
        relations: relations_out,
        truncated,
    })
}
