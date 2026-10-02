use loco_rs::controller::AppRoutes;

use crate::controllers;
use crate::controllers::route_inventory::{Edition, RouteClass, RouteInventory};

pub(super) fn community() -> (AppRoutes, RouteInventory) {
    let mut inventory = RouteInventory::default();
    inventory.add_allowlisted_exclusions();
    let mut app_routes = AppRoutes::with_default_routes();
    inventory.add_infrastructure(&app_routes);

    macro_rules! mount {
        ($route:expr) => {{
            let route = $route;
            let inventory_routes = AppRoutes::empty().add_route(route.clone());
            inventory.add_group(
                &inventory_routes,
                Edition::Community,
                false,
                RouteClass::Public,
            );
            app_routes = app_routes.add_route(route);
        }};
    }

    mount!(controllers::audit_log::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::audit_log::openapi_docs());
    mount!(controllers::api_keys::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::api_keys::openapi_docs());
    mount!(controllers::auth::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::auth::openapi_docs());
    mount!(controllers::entities::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::entities::openapi_docs());
    mount!(controllers::entities::migration_routes());
    mount!(controllers::export::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::export::openapi_docs());
    mount!(controllers::import::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::import::openapi_docs());
    mount!(controllers::members::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::members::openapi_docs());
    mount!(controllers::relations::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::relations::openapi_docs());
    mount!(controllers::schemas::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::schemas::openapi_docs());
    mount!(controllers::schemas::template_routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::schemas::template_openapi_docs());
    mount!(controllers::search::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::search::openapi_docs());
    mount!(controllers::setup::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::setup::openapi_docs());
    mount!(controllers::system::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::system::openapi_docs());
    mount!(controllers::template_library::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::template_library::openapi_docs());
    mount!(controllers::whoami::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::whoami::openapi_docs());
    mount!(controllers::workspaces::routes());
    #[cfg(feature = "openapi")]
    inventory.add_docs(controllers::workspaces::openapi_docs());

    (app_routes, inventory)
}
