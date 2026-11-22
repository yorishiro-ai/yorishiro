//! One table of stored strings for each closed set of values.
//!
//! A value like `"read_only"` used to be written out separately for `as_db_str`, `FromStr`, serde's `rename_all`, and the database `CHECK` constraint, and nothing but a round-trip test noticed when one copy drifted.
//! `db_enum!` takes the table once and derives every conversion from it, so the wire form and the stored form cannot disagree.
//!
//! Every conversion accepts only the canonical string.

/// Declares a closed enum whose variants each have one stored string.
///
/// ```ignore
/// db_enum! {
///     #[derive(Clone, Copy, PartialEq, Eq)]
///     pub enum Mode {
///         Off = "off",
///         ReadOnly = "read_only",
///     }
/// }
/// ```
///
/// Generates `ALL`, `as_db_str`, `from_db_str` (`None` for anything undefined, which callers should treat as a corrupt row rather than a missing value), `Display`, `FromStr`, `Serialize`, `Deserialize`, and `JsonSchema` from the same stored strings.
/// Serde derives and `#[serde(...)]` attributes must not be added: they would be a second copy of the table.
macro_rules! db_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident = $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $( $(#[$variant_meta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            $vis const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The string stored in the database and sent over the wire.
            #[must_use]
            $vis const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            /// The variant stored as `value`, or `None` for anything this crate does not define.
            #[must_use]
            $vis fn from_db_str(value: &str) -> Option<Self> {
                match value {
                    $( $text => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }

        impl ::schemars::JsonSchema for $name {
            fn schema_name() -> ::std::borrow::Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut ::schemars::SchemaGenerator) -> ::schemars::Schema {
                ::schemars::json_schema!({"type": "string", "enum": [$($text),+]})
            }
        }

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
