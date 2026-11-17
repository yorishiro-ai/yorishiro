//! The trust policy of an IAM role: which principals of the same tenant may assume it.
//!
//! It is the counterpart of an AWS role trust policy.
//! Permissions are not in this document; they come from the managed policies attached to the role.
//! Evaluation mirrors the permission policies: a matching deny wins, and without a matching allow the answer is no.

use serde::Deserialize;
use uuid::Uuid;

use crate::YorishiroError;
use crate::models::iam_policy::Effect;

const MAX_STATEMENTS: usize = 16;
const MAX_PRINCIPALS: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
/// A same-tenant principal a trust statement can name.
pub enum Principal {
    /// Every principal of the tenant, still subject to its own `role_assume` permission, like the AWS account root principal.
    Tenant,
    User {
        user_id: Uuid,
    },
    Group {
        group_id: Uuid,
    },
    /// A service credential (an API key without a user).
    ServiceKey {
        api_key_id: Uuid,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Statement {
    effect: Effect,
    principals: Vec<Principal>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    statements: Vec<Statement>,
}

/// Who is asking to assume a role.
pub struct Caller<'a> {
    pub user_id: Option<Uuid>,
    pub group_ids: &'a [Uuid],
    pub api_key_id: Uuid,
}

/// A validated trust policy.
#[derive(Debug)]
pub struct TrustPolicy {
    statements: Vec<Statement>,
}

fn invalid(message: &'static str) -> YorishiroError {
    YorishiroError::ValidationFailed {
        message: message.into(),
        details: vec![],
        hint: "Provide a trust policy with `statements`, each with an `effect` and `principals`"
            .into(),
    }
}

impl TrustPolicy {
    /// Parses and validates the shape of a trust document.
    /// Whether the named users, groups and service keys exist in the tenant is checked by the role model, which has the database.
    ///
    /// # Errors
    /// Returns a validation error for malformed or oversized documents.
    pub fn parse(document: &str) -> Result<Self, YorishiroError> {
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("trust policy is too large"));
        }
        let parsed: Document =
            serde_json::from_str(document).map_err(|_| invalid("invalid trust policy"))?;
        if parsed.statements.is_empty() || parsed.statements.len() > MAX_STATEMENTS {
            return Err(invalid("invalid statement count"));
        }
        if parsed
            .statements
            .iter()
            .any(|s| s.principals.is_empty() || s.principals.len() > MAX_PRINCIPALS)
        {
            return Err(invalid("invalid principal count"));
        }
        Ok(Self {
            statements: parsed.statements,
        })
    }

    /// Every principal the document names, for referential validation.
    #[must_use]
    pub fn principals(&self) -> Vec<Principal> {
        self.statements
            .iter()
            .flat_map(|s| s.principals.iter().copied())
            .collect()
    }

    fn matches(principal: Principal, caller: &Caller<'_>) -> bool {
        match principal {
            Principal::Tenant => true,
            Principal::User { user_id } => caller.user_id == Some(user_id),
            Principal::Group { group_id } => caller.group_ids.contains(&group_id),
            Principal::ServiceKey { api_key_id } => {
                caller.user_id.is_none() && caller.api_key_id == api_key_id
            }
        }
    }

    /// Whether the caller is trusted: some allow matches and no deny does.
    #[must_use]
    pub fn permits(&self, caller: &Caller<'_>) -> bool {
        let mut allowed = false;
        for statement in &self.statements {
            if statement
                .principals
                .iter()
                .any(|p| Self::matches(*p, caller))
            {
                if statement.effect == Effect::Deny {
                    return false;
                }
                allowed = true;
            }
        }
        allowed
    }

    /// Whether the document names this service key explicitly with an allow, which is what lets a caller with no identity policies assume the role.
    #[must_use]
    pub fn names_service_key(&self, api_key_id: Uuid) -> bool {
        self.statements.iter().any(|s| {
            s.effect == Effect::Allow
                && s.principals.iter().any(
                    |p| matches!(p, Principal::ServiceKey { api_key_id: id } if *id == api_key_id),
                )
        })
    }
}
