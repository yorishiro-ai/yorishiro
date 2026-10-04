use axum::http::Method;
use yorishiro::controllers::route_inventory::{RouteInventory, operation_id};

#[test]
fn operation_ids_are_stable_and_path_safe() {
    assert_eq!(
        operation_id(&Method::GET, "/api/entities/{id}"),
        "get_api_entities_id"
    );
    assert_eq!(operation_id(&Method::POST, "/setup"), "post_setup");
}

#[test]
fn allowlisted_non_api_paths_have_explicit_exclusions() {
    let mut inventory = RouteInventory::default();
    inventory.add_allowlisted_exclusions();

    assert_eq!(inventory.exclusions.len(), 4);
    assert!(
        inventory
            .exclusions
            .iter()
            .any(|item| item.path == "/mcp" && item.reason.contains("MCP"))
    );
    assert!(
        inventory
            .exclusions
            .iter()
            .any(|item| item.path == "/docs/openapi.json")
    );
}
