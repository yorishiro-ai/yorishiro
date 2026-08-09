use super::*;

/// `definition` and `template_id` are mutually exclusive and both optional at the type level,
/// so deserialization accepts either -- the exclusivity is enforced by the handler, and this
/// pins that the shape itself permits both spellings a client might send.
#[test]
fn a_schema_can_be_created_from_either_a_definition_or_a_template() {
    let inline: CreateSchemaArgs = serde_json::from_value(serde_json::json!({
        "definition": { "name": "s", "entity_types": {} }
    }))
    .unwrap();
    assert!(inline.definition.is_some());
    assert!(inline.template_id.is_none());

    let from_template: CreateSchemaArgs =
        serde_json::from_value(serde_json::json!({ "template_id": "general-notes" })).unwrap();
    assert!(from_template.definition.is_none());
    assert!(from_template.template_id.is_some());
}

/// Looking a schema up by name is the common agent path; the name is required.
#[test]
fn fetching_the_active_schema_requires_a_name() {
    assert!(serde_json::from_value::<GetActiveSchemaArgs>(serde_json::json!({})).is_err());

    let args: GetActiveSchemaArgs =
        serde_json::from_value(serde_json::json!({ "name": "task-management" })).unwrap();
    assert_eq!(args.name, "task-management");
}

/// Fetching by id takes a UUID, and a malformed one is rejected up front.
#[test]
fn fetching_by_id_rejects_a_malformed_uuid() {
    assert!(
        serde_json::from_value::<GetSchemaByIdArgs>(serde_json::json!({ "schema_id": "nope" }))
            .is_err()
    );
}
