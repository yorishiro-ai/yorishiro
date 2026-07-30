mod projection;
mod types;
mod validate;
mod versioning;

pub use projection::entity_type_to_json_schema;
pub use types::{
    ArrayItems, EntityTypeDef, FieldDef, FieldTypeName, MetaSchemaDefinition, RelationTypeDef,
};
pub use validate::validate_definition;
pub use versioning::{VersioningDiff, diff};

/// Re-exported only so the crate-root integration test in `tests/` can reference it;
/// `#[doc(hidden)]` keeps it out of the public API docs.
#[doc(hidden)]
pub use validate::MAX_OBJECT_DEPTH;
