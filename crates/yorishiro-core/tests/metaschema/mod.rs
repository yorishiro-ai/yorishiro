use super::*;

/// This module exists to re-export the metaschema surface under one path. The test pins that
/// surface: if a re-export is dropped, every downstream `metaschema::X` path breaks at once, and
/// the compiler error would otherwise point at the consumer rather than at the removal.
#[test]
fn the_public_surface_is_reachable_through_this_module() {
    fn assert_reachable<T>() {}

    assert_reachable::<MetaSchemaDefinition>();
    assert_reachable::<EntityTypeDef>();
    assert_reachable::<RelationTypeDef>();
    assert_reachable::<FieldDef>();
    assert_reachable::<FieldTypeName>();
    assert_reachable::<ArrayItems>();
    assert_reachable::<VersioningDiff>();

    let _: fn(&EntityTypeDef) -> serde_json::Value = entity_type_to_json_schema;
    assert!(std::hint::black_box(MAX_OBJECT_DEPTH) > 0);
}
