//! The conversions `db_enum!` derives, exercised through `MaintenanceMode`, the enum that declares `| "alias"` spellings.
//!
//! The macro itself stays crate-private, so these tests reach it through the types it generates.

use yorishiro::models::system_maintenance::MaintenanceMode;

#[test]
fn every_variant_round_trips_through_every_conversion() {
    for variant in MaintenanceMode::ALL {
        let text = variant.as_db_str();
        assert_eq!(MaintenanceMode::from_db_str(text), Some(*variant));
        assert_eq!(text.parse::<MaintenanceMode>(), Ok(*variant));
        assert_eq!(variant.to_string(), text);
        let json = serde_json::to_value(variant).unwrap();
        assert_eq!(json, serde_json::Value::String(text.into()));
        assert_eq!(
            serde_json::from_value::<MaintenanceMode>(json).unwrap(),
            *variant
        );
    }
}

#[test]
fn aliases_parse_but_are_not_part_of_the_wire_contract() {
    assert_eq!(
        MaintenanceMode::from_db_str("read-only"),
        Some(MaintenanceMode::ReadOnly)
    );
    assert_eq!(
        "read-only".parse::<MaintenanceMode>(),
        Ok(MaintenanceMode::ReadOnly)
    );
    assert!(serde_json::from_value::<MaintenanceMode>("read-only".into()).is_err());
}

#[test]
fn unknown_values_are_rejected_everywhere() {
    assert_eq!(MaintenanceMode::from_db_str("on"), None);
    assert_eq!(
        "on".parse::<MaintenanceMode>(),
        Err("unknown MaintenanceMode: on".to_string())
    );
    assert!(serde_json::from_value::<MaintenanceMode>("on".into()).is_err());
}
