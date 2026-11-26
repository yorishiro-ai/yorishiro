//! The metaschema: the definition language for entity and relation types, with its validation, JSON Schema projection and compatibility diff.

mod projection;
mod types;
mod validate;
mod versioning;

pub use projection::entity_type_to_json_schema;
pub use types::ArrayItemType;
pub use types::{
    ArrayItems, EntityTypeDef, FieldDef, FieldTypeName, MetaSchemaDefinition, RelationTypeDef,
};
pub use validate::validate_definition;
pub use versioning::{VersioningDiff, diff};

#[doc(hidden)]
pub use validate::MAX_OBJECT_DEPTH;
