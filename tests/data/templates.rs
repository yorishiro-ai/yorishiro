use yorishiro::data::templates::{TEMPLATES, parse};

#[test]
fn every_builtin_asset_is_a_schema_definition() {
    for template in TEMPLATES {
        let definition = parse(template);
        assert!(!definition.entity_types.is_empty(), "{}", template.id);
    }
}
