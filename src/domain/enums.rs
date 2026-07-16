//! Enumerations matching the frozen PostgreSQL enum types.
//!
//! Each enum corresponds to a PostgreSQL `CREATE TYPE` defined in migration 0001.
//! All variants are snake_case in the database and converted via `AsRef<str>` / `FromStr`.

#![allow(dead_code)]
#![allow(clippy::upper_case_acronyms)]

use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Helper macro
// ---------------------------------------------------------------------------

macro_rules! make_enum {
    ($name:ident, $display:literal, $(($variant:ident, $db:literal)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// All enum variants.
            pub const VARIANTS: &'static [Self] = &[$(Self::$variant),+];
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $(Self::$variant => write!(f, "{}", $db)),+
                }
            }
        }

        impl FromStr for $name {
            type Err = UnknownEnumVariant;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($db => Ok(Self::$variant)),+,
                    _ => Err(UnknownEnumVariant {
                        enum_name: stringify!($name).to_string(),
                        value: s.to_string(),
                    }),
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <&str as sqlx::Type<sqlx::Postgres>>::type_info()
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $name {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                let s = <&str as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
                Ok(s.parse()?)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut <sqlx::Postgres as sqlx::Database>::ArgumentBuffer<'q>,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
                let s: &str = &self.to_string();
                <&str as sqlx::Encode<'q, sqlx::Postgres>>::encode(s, buf)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UnknownEnumVariant {
    pub enum_name: String,
    pub value: String,
}

impl fmt::Display for UnknownEnumVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown variant '{}' for enum {}",
            self.value, self.enum_name
        )
    }
}

impl std::error::Error for UnknownEnumVariant {}

// ---------------------------------------------------------------------------
// Enums (values match the PostgreSQL enum labels)
// ---------------------------------------------------------------------------

make_enum!(
    PrincipalType,
    "principal_type",
    (HUMAN, "HUMAN"),
    (AGENT, "AGENT"),
    (SERVICE, "SERVICE"),
);

make_enum!(
    DefinitionVersionStatus,
    "definition_version_status",
    (DRAFT, "DRAFT"),
    (PUBLISHED, "PUBLISHED"),
    (DEPRECATED, "DEPRECATED"),
    (REVOKED, "REVOKED"),
);

make_enum!(
    NodeType,
    "node_type",
    (DRAFT, "DRAFT"),
    (NORMAL, "NORMAL"),
    (TERMINAL, "TERMINAL"),
);

make_enum!(
    AssigneeRefType,
    "assignee_ref_type",
    (WorkflowCreator, "WORKFLOW_CREATOR"),
    (DomainOwner, "DOMAIN_OWNER"),
    (FixedPrincipal, "FIXED_PRINCIPAL"),
);

make_enum!(
    TransitionEffect,
    "transition_effect",
    (Advance, "ADVANCE"),
    (Return, "RETURN"),
    (Terminate, "TERMINATE"),
);

make_enum!(
    ReceiptStatus,
    "receipt_status",
    (Processing, "PROCESSING"),
    (Completed, "COMPLETED"),
);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_display_and_parse() {
        for variant in PrincipalType::VARIANTS {
            let s = variant.to_string();
            let parsed: PrincipalType = s.parse().unwrap();
            assert_eq!(*variant, parsed);
        }
    }

    #[test]
    fn enum_unknown_variant() {
        let err = "INVALID".parse::<PrincipalType>().unwrap_err();
        assert!(err.to_string().contains("INVALID"));
    }

    #[test]
    fn definition_version_status_values() {
        assert_eq!(DefinitionVersionStatus::DRAFT.to_string(), "DRAFT");
        assert_eq!(DefinitionVersionStatus::PUBLISHED.to_string(), "PUBLISHED");
        assert_eq!(
            DefinitionVersionStatus::DEPRECATED.to_string(),
            "DEPRECATED"
        );
        assert_eq!(DefinitionVersionStatus::REVOKED.to_string(), "REVOKED");
    }

    #[test]
    fn receipt_status_transition_allowed() {
        // Only PROCESSING -> COMPLETED is allowed per DB triggers
        assert!(ReceiptStatus::Processing != ReceiptStatus::Completed);
    }
}
