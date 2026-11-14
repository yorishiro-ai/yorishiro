use uuid::Uuid;
use yorishiro::{
    YorishiroError,
    models::iam_policy::{Action, Policy, Resource},
};

const MAX_STATEMENTS: usize = 64;
const MAX_ACTIONS: usize = 32;
const MAX_RESOURCES: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;

fn owns_workspace(tenant_id: Uuid, workspace_id: Uuid) -> bool {
    tenant_id == tenant() && workspace_id == Uuid::from_u128(2)
}

fn parse(document: &str, tenant_id: Uuid) -> Result<Policy, YorishiroError> {
    Policy::parse(document, tenant_id, owns_workspace)
}

fn allows(policies: &[Policy], tenant_id: Uuid, action: Action, target: Resource) -> bool {
    Policy::allows(policies, tenant_id, action, target, owns_workspace)
}

fn tenant() -> Uuid {
    Uuid::from_u128(1)
}

fn workspace() -> Resource {
    Resource::Workspace {
        tenant_id: tenant(),
        workspace_id: Uuid::from_u128(2),
    }
}

fn document(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"statements":[{{"effect":"{effect}","actions":["{action}"],"resources":[{resource}]}}]}}"#
    )
}

fn all_workspaces() -> String {
    format!(r#"{{"type":"all_workspaces","tenant_id":"{}"}}"#, tenant())
}

#[test]
fn deny_overrides_allow_and_default_is_deny() {
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    let deny = parse(
        &document("deny", "entity_delete", &all_workspaces()),
        tenant(),
    )
    .unwrap();
    assert!(allows(&[allow], tenant(), Action::EntityGet, workspace()));
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    assert!(!allows(
        &[deny, allow],
        tenant(),
        Action::EntityDelete,
        workspace()
    ));
    assert!(!allows(&[], tenant(), Action::EntityGet, workspace()));
}

#[test]
fn tenant_boundaries_and_workspace_wildcard_are_explicit() {
    let policy = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    let other = Uuid::from_u128(3);
    assert!(!allows(&[policy], other, Action::EntityGet, workspace()));
    let policy = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    assert!(!allows(
        &[policy],
        tenant(),
        Action::TenantGet,
        Resource::Tenant {
            tenant_id: tenant()
        }
    ));
    assert!(parse(&document("allow", "*", &all_workspaces()), other).is_err());

    let other_resource = format!(r#"{{"type":"all_workspaces","tenant_id":"{other}"}}"#);
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    let foreign = parse(&document("deny", "*", &other_resource), other).unwrap();
    assert!(!allows(
        &[allow, foreign],
        tenant(),
        Action::EntityGet,
        workspace()
    ));
}

#[test]
fn invalid_documents_fail_closed() {
    assert!(matches!(
        parse("{}", tenant()),
        Err(YorishiroError::ValidationFailed { .. })
    ));
    for input in [
        r#"{"statements":[]}"#,
        r#"{"statements":[{"effect":"allow","actions":[],"resources":[]}]}"#,
        r#"{"statements":[{"effect":"allow","actions":["entity_*"],"resources":[]}]}"#,
        r#"{"statements":[{"effect":"allow","actions":["*"],"resources":[],"conditions":{}}]}"#,
    ] {
        assert!(parse(input, tenant()).is_err(), "{input}");
    }
    assert!(parse(&" ".repeat(MAX_DOCUMENT_BYTES + 1), tenant()).is_err());
    let statement = format!(
        r#"{{"effect":"allow","actions":["*"],"resources":[{}]}}"#,
        all_workspaces()
    );
    let oversized = format!(
        r#"{{"statements":[{}]}}"#,
        vec![statement; MAX_STATEMENTS + 1].join(",")
    );
    assert!(parse(&oversized, tenant()).is_err());
    let actions = vec!["\"entity_get\""; MAX_ACTIONS + 1].join(",");
    let resources = vec![all_workspaces(); MAX_RESOURCES + 1].join(",");
    let too_many_actions = format!(
        r#"{{"statements":[{{"effect":"allow","actions":[{actions}],"resources":[{}]}}]}}"#,
        all_workspaces()
    );
    let too_many_resources = format!(
        r#"{{"statements":[{{"effect":"allow","actions":["*"],"resources":[{resources}]}}]}}"#
    );
    assert!(parse(&too_many_actions, tenant()).is_err());
    assert!(parse(&too_many_resources, tenant()).is_err());
}

#[test]
fn actions_are_independent_and_restricted_to_their_resource_kind() {
    for (name, action) in [
        ("relation_create", Action::RelationCreate),
        ("schema_fork_update", Action::SchemaForkUpdate),
        ("import_create", Action::ImportCreate),
        ("migration_undo", Action::MigrationUndo),
        ("inference_confirm", Action::InferenceConfirm),
        ("embedding_provider_update", Action::EmbeddingProviderUpdate),
        ("audit_log_list", Action::AuditLogList),
    ] {
        let policy = parse(&document("allow", name, &all_workspaces()), tenant()).unwrap();
        assert!(allows(&[policy], tenant(), action, workspace()), "{name}");
        let tenant_resource = format!(r#"{{"type":"tenant","tenant_id":"{}"}}"#, tenant());
        assert!(parse(&document("allow", name, &tenant_resource), tenant()).is_err());
    }
    let tenant_resource = format!(r#"{{"type":"tenant","tenant_id":"{}"}}"#, tenant());
    for (name, action) in [
        ("credential_create", Action::CredentialCreate),
        ("group_member_add", Action::GroupMemberAdd),
        ("policy_delete", Action::PolicyDelete),
        ("builtin_template_list", Action::BuiltinTemplateList),
        ("builtin_template_get", Action::BuiltinTemplateGet),
    ] {
        let policy = parse(&document("allow", name, &tenant_resource), tenant()).unwrap();
        assert!(allows(
            &[policy],
            tenant(),
            action,
            Resource::Tenant {
                tenant_id: tenant()
            }
        ));
        assert!(parse(&document("allow", name, &all_workspaces()), tenant()).is_err());
    }
    let read = parse(
        &document("allow", "entity_get", &all_workspaces()),
        tenant(),
    )
    .unwrap();
    assert!(!allows(
        &[read],
        tenant(),
        Action::EntityDelete,
        workspace()
    ));
    let wildcard = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    assert!(!allows(
        &[wildcard],
        tenant(),
        Action::EntityGet,
        Resource::AllWorkspaces {
            tenant_id: tenant()
        }
    ));
}

#[test]
fn workspace_ownership_is_checked_at_parse_and_evaluation() {
    let workspace_id = Uuid::from_u128(2);
    let concrete = format!(
        r#"{{"type":"workspace","tenant_id":"{}","workspace_id":"{workspace_id}"}}"#,
        tenant()
    );
    let input = document("allow", "entity_get", &concrete);
    assert!(Policy::parse(&input, tenant(), |_, _| false).is_err());
    let policy = parse(&input, tenant()).unwrap();
    assert!(!Policy::allows(
        &[policy],
        tenant(),
        Action::EntityGet,
        workspace(),
        |_, _| false
    ));

    let wildcard = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    let foreign = Resource::Workspace {
        tenant_id: tenant(),
        workspace_id: Uuid::from_u128(3),
    };
    assert!(!allows(&[wildcard], tenant(), Action::EntityGet, foreign));
}

#[test]
fn stale_unrelated_selectors_do_not_block_grants_or_hide_denies() {
    let stale_id = Uuid::from_u128(3);
    let stale = format!(
        r#"{{"type":"workspace","tenant_id":"{}","workspace_id":"{stale_id}"}}"#,
        tenant()
    );
    let previously_owned = |tenant_id, _| tenant_id == tenant();
    let stale_allow = Policy::parse(
        &document("allow", "entity_get", &stale),
        tenant(),
        previously_owned,
    )
    .unwrap();
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    assert!(allows(
        &[stale_allow, allow],
        tenant(),
        Action::EntityGet,
        workspace()
    ));

    let tenant_resource = format!(r#"{{"type":"tenant","tenant_id":"{}"}}"#, tenant());
    let stale_deny = Policy::parse(
        &document("deny", "entity_get", &stale),
        tenant(),
        previously_owned,
    )
    .unwrap();
    let tenant_allow = parse(&document("allow", "tenant_get", &tenant_resource), tenant()).unwrap();
    assert!(allows(
        &[stale_deny, tenant_allow],
        tenant(),
        Action::TenantGet,
        Resource::Tenant {
            tenant_id: tenant()
        }
    ));

    let stale_deny = Policy::parse(
        &document("deny", "entity_get", &stale),
        tenant(),
        previously_owned,
    )
    .unwrap();
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    let target = Resource::Workspace {
        tenant_id: tenant(),
        workspace_id: stale_id,
    };
    assert!(!allows(
        &[stale_deny, allow],
        tenant(),
        Action::EntityGet,
        target
    ));
    let stale_deny = Policy::parse(
        &document("deny", "entity_get", &stale),
        tenant(),
        previously_owned,
    )
    .unwrap();
    let allow = parse(&document("allow", "*", &all_workspaces()), tenant()).unwrap();
    assert!(!Policy::allows(
        &[stale_deny, allow],
        tenant(),
        Action::EntityGet,
        target,
        previously_owned
    ));
}
