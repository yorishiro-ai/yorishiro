//! Names and YRNs of users, groups and access keys.

use uuid::Uuid;

use crate::YorishiroError;

pub(crate) const MAX_USER_NAME_CHARS: usize = 64;
pub(crate) const MAX_GROUP_NAME_CHARS: usize = 128;
pub(crate) const MAX_POLICY_NAME_CHARS: usize = 128;

const NAME_CHARSET: &str = "letters, digits and + = , . @ _ -";

/// Checks a name against its length limit and the shared character set.
///
/// # Errors
/// Returns a validation error naming `kind` when the name is empty, too long or holds another character.
pub(crate) fn validate_name(kind: &str, name: &str, max: usize) -> Result<(), YorishiroError> {
    let valid = !name.is_empty()
        && name.chars().count() <= max
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '=' | ',' | '.' | '@' | '_' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err(YorishiroError::ValidationFailed {
            message: format!("{kind} name must contain 1 to {max} characters: {NAME_CHARSET}"),
            details: vec![],
            hint: format!("use only {NAME_CHARSET}"),
        })
    }
}

/// The key two names collide on: unique names ignore case.
pub(crate) fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

pub(crate) fn user_yrn(tenant_id: Uuid, user_name: &str) -> String {
    format!("yrn:iam:{tenant_id}:user/{user_name}")
}

/// The YRN of a tenant's root user, which has no name.
pub(crate) fn root_yrn(tenant_id: Uuid) -> String {
    format!("yrn:iam:{tenant_id}:root")
}

/// The YRN of an access key, which names a caller that has no user.
pub(crate) fn access_key_yrn(tenant_id: Uuid, access_key_id: Uuid) -> String {
    format!("yrn:iam:{tenant_id}:access-key/{access_key_id}")
}

pub(crate) fn group_yrn(tenant_id: Uuid, name: &str) -> String {
    format!("yrn:iam:{tenant_id}:group/{name}")
}
