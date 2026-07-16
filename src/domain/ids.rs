//! Strongly-typed ID newtypes for all workflow domain entities.
//!
//! Each ID wraps a [`Uuid`] and prevents accidental mixing between
//! different entity types at compile time.

use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Macro: generate a newtype ID with From, Display, and sqlx support
// ---------------------------------------------------------------------------

macro_rules! make_id {
    ($name:ident, $display:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Create a new random ID.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing [`Uuid`].
            pub fn from_uuid(u: Uuid) -> Self {
                Self(u)
            }

            /// Return the inner [`Uuid`].
            pub fn into_uuid(self) -> Uuid {
                self.0
            }

            /// Borrow the inner [`Uuid`].
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s).map(Self)
            }
        }

        // sqlx integration: encode/decode as UUID
        impl sqlx::Type<sqlx::Postgres> for $name {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <Uuid as sqlx::Type<sqlx::Postgres>>::type_info()
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $name {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                Uuid::decode(value).map(Self)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $name {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>> {
                self.0.encode_by_ref(buf)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Identity & Domain
// ---------------------------------------------------------------------------

make_id!(PrincipalId, "principal_id");
make_id!(DomainId, "domain_id");
make_id!(BindingId, "binding_id");

// ---------------------------------------------------------------------------
// Definition
// ---------------------------------------------------------------------------

make_id!(WorkflowDefinitionId, "workflow_definition_id");
make_id!(DefinitionVersionId, "definition_version_id");
make_id!(NodeId, "node_id");
make_id!(TransitionId, "transition_id");

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

make_id!(WorkflowInstanceId, "workflow_instance_id");
make_id!(ContextRevisionId, "context_revision_id");
make_id!(NodeVisitId, "node_visit_id");
make_id!(SubmissionId, "submission_id");

// ---------------------------------------------------------------------------
// Events & Commands
// ---------------------------------------------------------------------------

make_id!(EventId, "event_id");
make_id!(CommandId, "command_id");
make_id!(AttemptAuditId, "attempt_audit_id");
make_id!(SecurityAuditId, "security_audit_id");

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_types() {
        // Ensure different ID types don't accidentally equal each other
        let inst = WorkflowInstanceId::new();
        let visit = NodeVisitId::new();
        let _ = (inst, visit); // Just checking they compile as different types
    }

    #[test]
    fn id_roundtrip() {
        let original = WorkflowInstanceId::new();
        let uuid = original.into_uuid();
        let restored = WorkflowInstanceId::from_uuid(uuid);
        assert_eq!(original, restored);
    }

    #[test]
    fn id_display_and_parse() {
        let id = PrincipalId::new();
        let s = id.to_string();
        let parsed: PrincipalId = s.parse().expect("should parse valid UUID");
        assert_eq!(id, parsed);
    }
}
