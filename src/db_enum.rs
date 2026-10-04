//! One table of stored strings for each closed set of values.
//!
//! A value like `"read_only"` used to be written out separately for `as_db_str`, `FromStr`, serde's `rename_all`, and the database `CHECK` constraint, and nothing but a round-trip test noticed when one copy drifted.
//! [`db_enum!`] takes the table once and derives every conversion from it, so the wire form and the stored form cannot disagree.
//!
//! The generated `Deserialize` accepts only the canonical string.
//! `from_db_str` and `FromStr` also accept the `| "alias"` spellings, which exist for command-line input and are not part of the stored or wire contract.

/// Declares a closed enum whose variants each have one stored string.
///
/// ```ignore
/// db_enum! {
///     #[derive(Clone, Copy, Debug, PartialEq, Eq)]
///     pub enum Mode {
///         Off = "off",
///         ReadOnly = "read_only" | "read-only",
///     }
/// }
/// ```
///
/// Generates `ALL`, `as_db_str`, `from_db_str` (`None` for anything undefined, which callers should treat as a corrupt row rather than a missing value), `Display`, `FromStr`, `Serialize`, `Deserialize`, and, with the `openapi` feature, the OpenAPI string-enum schema.
/// Serde derives and `#[serde(...)]` attributes must not be added: they would be a second copy of the table.
macro_rules! db_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident = $text:literal $(| $alias:literal)* ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The string stored in the database and sent over the wire.
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// The variant stored as `value`, or `None` for anything this crate does not define.
            #[must_use]
            pub fn from_db_str(value: &str) -> Option<Self> {
                match value {
                    $( $text $(| $alias)* => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }

        // The contract is derived from the same table as every other conversion, so the documented
        // values cannot drift from the stored ones.
        #[cfg(feature = "openapi")]
        impl ::utoipa::PartialSchema for $name {
            fn schema() -> ::utoipa::openapi::RefOr<::utoipa::openapi::schema::Schema> {
                ::utoipa::openapi::schema::ObjectBuilder::new()
                    .schema_type(::utoipa::openapi::schema::SchemaType::new(
                        ::utoipa::openapi::schema::Type::String,
                    ))
                    .enum_values(Some([$($text),+]))
                    .into()
            }
        }

        #[cfg(feature = "openapi")]
        impl ::utoipa::ToSchema for $name {}

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.as_db_str())
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = String;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_db_str(value)
                    .ok_or_else(|| format!("unknown {}: {value}", stringify!($name)))
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_db_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <String as ::serde::Deserialize>::deserialize(deserializer)?;
                Self::ALL
                    .iter()
                    .copied()
                    .find(|variant| variant.as_db_str() == value)
                    .ok_or_else(|| {
                        <D::Error as ::serde::de::Error>::unknown_variant(&value, &[$($text),+])
                    })
            }
        }
    };
}

pub(crate) use db_enum;
