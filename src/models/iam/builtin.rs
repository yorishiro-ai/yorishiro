//! The managed policies every tenant can attach: `AdministratorAccess`, `PowerUserAccess` and `ReadOnlyAccess`.
//!
//! The product defines their documents in code, so a release that adds an action updates them everywhere at once.
//! A managed policy has no row: it has a fixed id and one virtual version generated from the code, and evaluation uses that version.
//! A managed policy cannot be created, versioned or deleted by a tenant.

use serde_json::json;
use uuid::Uuid;

use crate::error::{ResultExt, YorishiroError};
use crate::models::iam::policy::{AccessLevel, Action, Policy, Service, VERSION};

pub(crate) const ADMINISTRATOR_ACCESS: &str = "AdministratorAccess";
pub(crate) const POWER_USER_ACCESS: &str = "PowerUserAccess";
pub(crate) const READ_ONLY_ACCESS: &str = "ReadOnlyAccess";
pub(crate) const WORKSPACE_VIEWER_ACCESS: &str = "WorkspaceViewerAccess";
pub(crate) const WORKSPACE_CONTRIBUTOR_ACCESS: &str = "WorkspaceContributorAccess";

pub(crate) const MANAGED_POLICY_NAMES: [&str; 5] = [
    ADMINISTRATOR_ACCESS,
    POWER_USER_ACCESS,
    READ_ONLY_ACCESS,
    WORKSPACE_VIEWER_ACCESS,
    WORKSPACE_CONTRIBUTOR_ACCESS,
];

/// The fixed id of each managed policy, the same in every tenant.
const MANAGED_POLICY_IDS: [(&str, Uuid); 5] = [
    (
        ADMINISTRATOR_ACCESS,
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001),
    ),
    (
        POWER_USER_ACCESS,
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0002),
    ),
    (
        READ_ONLY_ACCESS,
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0003),
    ),
    (
        WORKSPACE_VIEWER_ACCESS,
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0004),
    ),
    (
        WORKSPACE_CONTRIBUTOR_ACCESS,
        Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0005),
    ),
];

/// The id a managed policy is attached and referred to by.
#[must_use]
pub fn managed_policy_id(name: &str) -> Option<Uuid> {
    MANAGED_POLICY_IDS
        .iter()
        .find(|(managed, _)| *managed == name)
        .map(|(_, id)| *id)
}

/// The name of the managed policy with this id.
pub(crate) fn managed_policy_name(id: Uuid) -> Option<&'static str> {
    MANAGED_POLICY_IDS
        .iter()
        .find(|(_, managed)| *managed == id)
        .map(|(name, _)| *name)
}

/// The services that act on the data of a workspace.
///
/// A managed policy applies to every workspace; to limit a user to one, create a policy with the same actions and the workspace YRN as its resource.
const WORKSPACE_DATA_SERVICES: [Service; 4] = [
    Service::Workspace,
    Service::Schema,
    Service::Entity,
    Service::Migration,
];

pub(crate) fn is_managed_policy_name(name: &str) -> bool {
    MANAGED_POLICY_NAMES.contains(&name)
}

/// The YRN a policy is attached and referred to by; a managed policy has an empty tenant field.
pub(crate) fn policy_yrn(tenant_id: Uuid, name: &str) -> String {
    if is_managed_policy_name(name) {
        format!("yrn:iam::policy/{name}")
    } else {
        format!("yrn:iam:{tenant_id}:policy/{name}")
    }
}

/// The document of a managed policy, or `None` when `name` is not one.
pub(crate) fn managed_document(name: &str) -> Option<serde_json::Value> {
    let statement = match name {
        ADMINISTRATOR_ACCESS => {
            json!({"Effect": "Allow", "Action": "*", "Resource": "*"})
        }
        POWER_USER_ACCESS => json!({
            "Effect": "Allow",
            "NotAction": [
                format!("{}:*", Service::Iam.prefix()),
                format!("{}:*", Service::Tenant.prefix()),
            ],
            "Resource": "*",
        }),
        READ_ONLY_ACCESS => {
            let reads: Vec<String> = Action::ALL
                .iter()
                .filter(|action| {
                    matches!(action.access_level(), AccessLevel::List | AccessLevel::Read)
                })
                .map(|action| action.qualified())
                .collect();
            json!({"Effect": "Allow", "Action": reads, "Resource": "*"})
        }
        WORKSPACE_VIEWER_ACCESS => {
            let reads: Vec<String> = Action::ALL
                .iter()
                .filter(|action| {
                    WORKSPACE_DATA_SERVICES.contains(&action.service())
                        && matches!(action.access_level(), AccessLevel::List | AccessLevel::Read)
                })
                .map(|action| action.qualified())
                .collect();
            json!({"Effect": "Allow", "Action": reads, "Resource": "*"})
        }
        WORKSPACE_CONTRIBUTOR_ACCESS => {
            let work: Vec<String> = Action::ALL
                .iter()
                .filter(|action| match action.service() {
                    Service::Workspace => {
                        matches!(action.access_level(), AccessLevel::List | AccessLevel::Read)
                    }
                    service => WORKSPACE_DATA_SERVICES.contains(&service),
                })
                .map(|action| action.qualified())
                .collect();
            json!({"Effect": "Allow", "Action": work, "Resource": "*"})
        }
        _ => return None,
    };
    Some(json!({"Version": VERSION, "Statement": [statement]}))
}

/// The parsed policy of a managed name, or `None` when `name` is not one.
///
/// # Errors
/// Returns a validation error when the generated document does not parse, which is a defect.
pub(crate) fn managed_policy(
    name: &str,
    tenant_id: Uuid,
) -> Result<Option<Policy>, YorishiroError> {
    let Some(document) = managed_document(name) else {
        return Ok(None);
    };
    let rendered = serde_json::to_string(&document).internal()?;
    Policy::parse_generated(&rendered, tenant_id).map(Some)
}
