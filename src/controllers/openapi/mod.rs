//! The `OpenAPI` contract: route inventory, served documents and the schema shapes they name.

#[cfg(feature = "openapi")]
mod schemas;
#[cfg(feature = "openapi")]
pub use schemas::*;

pub mod route_inventory;
pub mod swagger;
