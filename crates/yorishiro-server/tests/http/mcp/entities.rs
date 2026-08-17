use super::*;

/// MCP tool arguments are deserialized straight from what an LLM client sends, so the optional fields have to actually be optional: a client omitting `filter`/`limit`/`offset` is the common case, and a required-by-accident field would fail every such call.
#[test]
fn listing_arguments_accept_an_empty_object() {
    let args: ListEntitiesArgs = serde_json::from_value(serde_json::json!({})).unwrap();

    assert!(args.entity_type.is_none());
    assert!(args.filter.is_none());
    assert!(args.limit.is_none());
    assert!(args.offset.is_none());
}

/// Conversely the create arguments are all required: an entity with no schema or type cannot be placed, so a client omitting them must get a deserialization error rather than a default.
#[test]
fn create_arguments_require_schema_type_and_body() {
    assert!(serde_json::from_value::<CreateEntityArgs>(serde_json::json!({})).is_err());
    assert!(
        serde_json::from_value::<CreateEntityArgs>(serde_json::json!({ "schema_name": "s" }))
            .is_err()
    );

    let args: CreateEntityArgs = serde_json::from_value(serde_json::json!({
        "schema_name": "task-management",
        "entity_type": "task",
        "data": { "title": "write tests" }
    }))
    .unwrap();

    assert_eq!(args.schema_name, "task-management");
    assert_eq!(args.entity_type, "task");
    assert_eq!(args.data["title"], "write tests");
}

/// Ids arrive as strings over JSON and must parse as UUIDs: a malformed id is a client error, not something to pass through to the query layer.
#[test]
fn id_arguments_reject_a_malformed_uuid() {
    assert!(
        serde_json::from_value::<GetEntityArgs>(serde_json::json!({ "id": "not-a-uuid" })).is_err()
    );

    let args: GetEntityArgs =
        serde_json::from_value(serde_json::json!({ "id": "00000000-0000-0000-0000-000000000000" }))
            .unwrap();
    assert!(args.id.is_nil());
}
