//! `labeled_enum!`: fieldless enums with a stable wire name and a human label.
//!
//! The wire name is used by serde, `FromStr` and `as_str` (and therefore in D1, JSON and YAML);
//! the label is what the UI shows (`Display`). Exported so the catalog crate's spec enums share
//! the same shape. Callers must depend on `serde` and `schemars`.

use std::fmt;

/// A string did not match any wire name of a [`labeled_enum!`] type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEnumError {
    /// Name of the enum type that failed to parse.
    pub type_name: &'static str,
    /// The rejected input.
    pub value: String,
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} `{}`", self.type_name, self.value)
    }
}

impl std::error::Error for ParseEnumError {}

/// Defines a `Copy` enum whose variants map to a wire name and a label.
///
/// ```
/// techdepo_domain::labeled_enum! {
///     /// Example.
///     pub enum Size {
///         Small => ("small", "Small"),
///         Large => ("large", "Large"),
///     }
/// }
/// assert_eq!("large".parse::<Size>().unwrap(), Size::Large);
/// assert_eq!(Size::Small.to_string(), "Small");
/// ```
#[macro_export]
macro_rules! labeled_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => ($wire:literal, $label:literal) ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            ::serde::Serialize, ::serde::Deserialize, ::schemars::JsonSchema,
        )]
        $vis enum $name {
            $( $(#[$vmeta])* #[serde(rename = $wire)] $variant, )+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Stable wire name (serde, D1, YAML).
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $wire,)+ }
            }

            /// Human-readable label for the UI.
            pub const fn label(self) -> &'static str {
                match self { $(Self::$variant => $label,)+ }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.label())
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::ParseEnumError;

            fn from_str(s: &str) -> ::std::result::Result<Self, Self::Err> {
                match s {
                    $($wire => Ok(Self::$variant),)+
                    _ => Err($crate::ParseEnumError {
                        type_name: stringify!($name),
                        value: s.to_owned(),
                    }),
                }
            }
        }
    };
}
