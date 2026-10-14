//! Opaque IDs that only exist at the API and storage layers (the domain ones are re-exported
//! from `techdepo_domain`).

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! opaque_id {
    ($( $(#[$meta:meta])* $name:ident; )+) => {$(
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps an existing identifier.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Unwraps the identifier.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    )+};
}

opaque_id! {
    /// Saved PC build (`bld_…`).
    BuildId;
    /// Payment ledger entry: a charge or a refund (`pay_…`).
    PaymentId;
    /// Saved customer address (`adr_…`).
    AddressId;
    /// `email_log` row (`eml_…`).
    EmailId;
    /// Agent or classifier trace (`run_…`).
    AgentRunId;
}
