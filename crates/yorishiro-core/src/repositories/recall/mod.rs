use std::collections::HashSet;

use serde_json::{Map, Value};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::models::entities::EntityRecord;
use crate::repositories::entities;
use crate::repositories::relations;
use crate::repositories::schemas;

pub use crate::models::recall::*;

/// Reduces `entity.data` down to only the fields marked `x-embed` in its entity_type
/// definition. Falls back to an empty body if the entity's schema version no longer defines
/// that entity_type at all, rather than failing the whole recall for one neighbor.
async fn shallow_copy(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    mut entity: EntityRecord,
) -> Result<EntityRecord, YorishiroError> {
    let schema = schemas::get_by_id(conn, workspace_id, entity.schema_id).await?;
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
    Ok(entity)
}

/// Fetches an entity's full body together with its relations and connected neighbors in one
/// call, so a caller doesn't need `entity_get` + `list_relations` + `entity_get` per neighbor
/// round trips. Neighbors are shallow (only `x-embed` fields) unless `query.full` is set.
///
/// `query.depth` controls how many hops are traversed outward from `entity_id` (clamped to
/// `[1, MAX_RECALL_DEPTH]`). At depth 1 this is exactly the original single-hop behavior: only
/// `entity_id`'s direct neighbors are returned. At depth > 1, each subsequent hop fetches the
/// neighbors of every entity discovered at the previous hop, expanding breadth-first. An entity
/// reachable by more than one path is only reported once, tagged with the shortest
/// `hop_distance` it was reached at, and never re-expanded once visited (so cycles terminate).
pub async fn recall_context(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    entity_id: Uuid,
    query: RecallQuery,
) -> Result<RecallContext, YorishiroError> {
    let limit = query.limit.clamp(1, 200);
    let depth = query.depth.clamp(1, MAX_RECALL_DEPTH);
    let full = query.full;

    let entity = entities::get(conn, workspace_id, entity_id).await?;

    // BFS outward from entity_id. `visited` tracks every entity already seen (the root plus
    // every neighbor emitted so far) so a given entity is only ever expanded/reported once, at
    // the hop it was first reached -- this both dedups diamond-shaped paths and guarantees
    // termination in the presence of cycles.
    let mut visited: HashSet<Uuid> = HashSet::from([entity_id]);
    let mut frontier: Vec<Uuid> = vec![entity_id];
    let mut relations_out = Vec::new();
    let mut truncated = false;

    for hop in 1..=depth {
        let mut next_frontier = Vec::new();

        for &source_id in &frontier {
            let mut neighbors =
                relations::neighbors(conn, workspace_id, source_id, limit + 1).await?;
            if neighbors.len() as i64 > limit {
                truncated = true;
            }
            neighbors.truncate(limit as usize);

            for neighbor in neighbors {
                if !visited.insert(neighbor.entity.id) {
                    continue;
                }
                next_frontier.push(neighbor.entity.id);

                let neighbor_entity = if full {
                    neighbor.entity
                } else {
                    shallow_copy(conn, workspace_id, neighbor.entity).await?
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
