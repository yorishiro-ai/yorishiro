use uuid::Uuid;
use yorishiro::{
    YorishiroError,
    models::iam::policy::{Action, Decision, Policy, Resource},
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

fn tenant_resource() -> Resource {
    Resource::Tenant {
        tenant_id: tenant(),
    }
}

fn tenant_yrn() -> String {
    format!("yrn:tenant:{}", tenant())
}

fn all_workspaces_arn() -> String {
    format!("yrn:workspace:{}:*", tenant())
}

fn workspace_yrn() -> String {
    format!("yrn:workspace:{}:{}", tenant(), Uuid::from_u128(2))
}

fn document(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

#[test]
fn deny_overrides_allow_and_default_is_deny() {
    let allow = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    let deny = parse(&document("Deny", "entity:DeleteEntity", "*"), tenant()).unwrap();
    assert!(allows(&[allow], tenant(), Action::GetEntity, workspace()));
    let allow = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    assert!(!allows(
        &[deny, allow],
        tenant(),
        Action::DeleteEntity,
        workspace()
    ));
    assert!(!allows(&[], tenant(), Action::GetEntity, workspace()));
}

#[test]
fn the_version_must_be_the_supported_one() {
    for input in [
        r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
        r#"{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
    ] {
        assert!(parse(input, tenant()).is_err(), "{input}");
    }
}

#[test]
fn a_statement_may_be_a_single_object_and_selectors_may_be_single_strings() {
    let policy = parse(
        r#"{"Version":"2012-10-17","Statement":{"Sid":"Read","Effect":"Allow","Action":"entity:GetEntity","Resource":"*"}}"#,
        tenant(),
    )
    .unwrap();
    assert!(allows(&[policy], tenant(), Action::GetEntity, workspace()));
}

#[test]
fn policy_validation_identifies_the_invalid_statement() {
    let policy = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "entity:GetEntity", "Resource": "*"},
            {"Effect": "Allow", "Action": "unknown:GetEntity", "Resource": "*"},
        ]
    });
    let error = parse(&policy.to_string(), tenant()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Statement[1]: action names an unknown service")
    );
}

#[test]
fn action_wildcards_select_by_service_and_name() {
    let service = parse(&document("Allow", "iam:*", "*"), tenant()).unwrap();
    assert!(allows(
        &[service],
        tenant(),
        Action::CreateGroup,
        tenant_resource()
    ));
    let service = parse(&document("Allow", "iam:*", "*"), tenant()).unwrap();
    assert!(!allows(
        &[service],
        tenant(),
        Action::GetEntity,
        workspace()
    ));

    let prefix = parse(&document("Allow", "iam:List*", "*"), tenant()).unwrap();
    assert!(allows(
        &[prefix],
        tenant(),
        Action::ListUsers,
        tenant_resource()
    ));
    let prefix = parse(&document("Allow", "iam:List*", "*"), tenant()).unwrap();
    assert!(!allows(
        &[prefix],
        tenant(),
        Action::CreateUser,
        tenant_resource()
    ));

    let single = parse(&document("Allow", "iam:Get?ser", "*"), tenant()).unwrap();
    assert!(allows(
        &[single],
        tenant(),
        Action::GetUser,
        tenant_resource()
    ));

    let ignores_case = parse(&document("Allow", "IAM:listusers", "*"), tenant()).unwrap();
    assert!(allows(
        &[ignores_case],
        tenant(),
        Action::ListUsers,
        tenant_resource()
    ));
}

#[test]
fn unknown_services_and_unmatched_names_are_rejected() {
    for action in [
        "nothing:*",
        "iam:NoSuchAction",
        "iam",
        "iam:",
        "entity:Nope*",
    ] {
        assert!(
            parse(&document("Allow", action, "*"), tenant()).is_err(),
            "{action}"
        );
    }
}

#[test]
fn not_action_grants_everything_but_the_named_actions() {
    let policy = parse(
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":["iam:*","tenant:*"],"Resource":"*"}]}"#,
        tenant(),
    )
    .unwrap();
    let policies = [policy];
    assert!(allows(&policies, tenant(), Action::GetEntity, workspace()));
    assert!(allows(
        &policies,
        tenant(),
        Action::ListWorkspaces,
        tenant_resource()
    ));
    assert!(!allows(
        &policies,
        tenant(),
        Action::CreateGroup,
        tenant_resource()
    ));
    assert!(!allows(
        &policies,
        tenant(),
        Action::UpdateTenant,
        tenant_resource()
    ));
}

#[test]
fn a_statement_needs_exactly_one_of_action_and_not_action() {
    for input in [
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Resource":"*"}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","NotAction":"iam:*","Resource":"*"}]}"#,
    ] {
        assert!(parse(input, tenant()).is_err(), "{input}");
    }
}

#[test]
fn arns_scope_a_statement_to_the_tenant_or_to_workspaces() {
    let tenant_only = parse(
        &document("Allow", "iam:CreateGroup", &tenant_yrn()),
        tenant(),
    )
    .unwrap();
    assert!(allows(
        &[tenant_only],
        tenant(),
        Action::CreateGroup,
        tenant_resource()
    ));

    let all = parse(
        &document("Allow", "entity:GetEntity", &all_workspaces_arn()),
        tenant(),
    )
    .unwrap();
    assert!(allows(&[all], tenant(), Action::GetEntity, workspace()));

    let one = parse(
        &document("Allow", "entity:GetEntity", &workspace_yrn()),
        tenant(),
    )
    .unwrap();
    assert!(allows(&[one], tenant(), Action::GetEntity, workspace()));
    let other_workspace = Resource::Workspace {
        tenant_id: tenant(),
        workspace_id: Uuid::from_u128(5),
    };
    let one = parse(
        &document("Allow", "entity:GetEntity", &workspace_yrn()),
        tenant(),
    )
    .unwrap();
    assert!(!Policy::allows(
        &[one],
        tenant(),
        Action::GetEntity,
        other_workspace,
        |_, _| true
    ));
}

#[test]
fn an_arn_of_the_wrong_kind_for_the_action_is_rejected() {
    assert!(
        parse(
            &document("Allow", "iam:CreateGroup", &all_workspaces_arn()),
            tenant()
        )
        .is_err()
    );
    assert!(
        parse(
            &document("Allow", "entity:GetEntity", &tenant_yrn()),
            tenant()
        )
        .is_err()
    );
}

#[test]
fn malformed_and_foreign_arns_are_rejected() {
    let foreign = format!("yrn:tenant:{}", Uuid::from_u128(9));
    for resource in [
        foreign.as_str(),
        "yrn:tenant::not-a-uuid:tenant",
        "yrn:tenant:eu:00000000-0000-0000-0000-000000000001:tenant",
        "yrn:workspace::00000000-0000-0000-0000-000000000001:workspace/nope",
        "yrn:elsewhere::00000000-0000-0000-0000-000000000001:tenant",
        "tenant",
    ] {
        assert!(
            parse(&document("Allow", "iam:CreateGroup", resource), tenant()).is_err(),
            "{resource}"
        );
    }
}

#[test]
fn tenant_boundaries_are_explicit() {
    let policy = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    let other = Uuid::from_u128(3);
    assert!(!allows(&[policy], other, Action::GetEntity, workspace()));
    let policy = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    assert!(allows(
        &[policy],
        tenant(),
        Action::GetTenant,
        tenant_resource()
    ));
    let allow = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    let foreign = parse(&document("Deny", "*", "*"), other).unwrap();
    assert!(!allows(
        &[allow, foreign],
        tenant(),
        Action::GetEntity,
        workspace()
    ));
}

#[test]
fn workspace_ownership_is_checked_at_parse_and_evaluation() {
    let unowned = format!("yrn:workspace:{}:{}", tenant(), Uuid::from_u128(7));
    assert!(parse(&document("Allow", "entity:GetEntity", &unowned), tenant()).is_err());
    let policy = parse(&document("Allow", "*", "*"), tenant()).unwrap();
    assert!(!Policy::allows(
        &[policy],
        tenant(),
        Action::GetEntity,
        workspace(),
        |_, _| false
    ));
}

#[test]
fn stale_unrelated_selectors_do_not_block_grants_or_hide_denies() {
    let ownership = |tenant_id: Uuid, _| tenant_id == tenant();
    let stale = format!("yrn:workspace:{}:{}", tenant(), Uuid::from_u128(8));
    let document = format!(
        r#"{{"Version":"2012-10-17","Statement":[
            {{"Effect":"Allow","Action":"entity:GetEntity","Resource":["{stale}","{}"]}},
            {{"Effect":"Deny","Action":"entity:DeleteEntity","Resource":"{stale}"}}
        ]}}"#,
        workspace_yrn()
    );
    let policy = Policy::parse(&document, tenant(), ownership).unwrap();
    assert!(Policy::allows(
        &[policy],
        tenant(),
        Action::GetEntity,
        workspace(),
        |_, id| id == Uuid::from_u128(2)
    ));
}

#[test]
fn invalid_documents_fail_closed() {
    assert!(matches!(
        parse("{}", tenant()),
        Err(YorishiroError::ValidationFailed { .. })
    ));
    for input in [
        r#"{"Version":"2012-10-17","Statement":[]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":[],"Resource":"*"}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":[]}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Maybe","Action":"*","Resource":"*"}]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*","Condition":{}}]}"#,
        r#"{"statements":[{"effect":"allow","actions":["*"],"resources":[]}]}"#,
    ] {
        assert!(parse(input, tenant()).is_err(), "{input}");
    }
    assert!(parse(&" ".repeat(MAX_DOCUMENT_BYTES + 1), tenant()).is_err());
    let statement = r#"{"Effect":"Allow","Action":"*","Resource":"*"}"#;
    let oversized = format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        vec![statement; MAX_STATEMENTS + 1].join(",")
    );
    assert!(parse(&oversized, tenant()).is_err());
    let actions = vec!["\"entity:GetEntity\""; MAX_ACTIONS + 1].join(",");
    let resources = vec!["\"*\""; MAX_RESOURCES + 1].join(",");
    let too_many_actions = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{actions}],"Resource":"*"}}]}}"#
    );
    let too_many_resources = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"*","Resource":[{resources}]}}]}}"#
    );
    assert!(parse(&too_many_actions, tenant()).is_err());
    assert!(parse(&too_many_resources, tenant()).is_err());
}

#[test]
fn sids_are_alphanumeric() {
    let with_sid = |sid: &str| {
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{sid}","Effect":"Allow","Action":"*","Resource":"*"}}]}}"#
        )
    };
    assert!(parse(&with_sid("ReadOnly1"), tenant()).is_ok());
    assert!(parse(&with_sid("not valid"), tenant()).is_err());
}

#[test]
fn every_action_has_a_unique_qualified_name_that_parses_back() {
    let mut seen = std::collections::HashSet::new();
    for action in Action::ALL {
        let qualified = action.qualified();
        assert!(seen.insert(qualified.clone()), "{qualified}");
        assert_eq!(Action::parse(&qualified), Some(*action));
        assert_eq!(Action::from_name(action.name()), Some(*action));
    }
}

#[test]
fn simulation_reports_the_decision_and_the_policies_behind_it() {
    let allow = parse(&document("Allow", "iam:List*", "*"), tenant()).unwrap();
    let deny = parse(&document("Deny", "iam:ListUsers", "*"), tenant()).unwrap();
    let policies = [allow, deny];

    let denied = Policy::simulate(&policies, tenant(), Action::ListUsers, tenant_resource());
    assert_eq!(denied.decision, Decision::ExplicitDeny);
    assert_eq!(denied.matched, vec![1]);

    let allowed = Policy::simulate(&policies, tenant(), Action::ListGroups, tenant_resource());
    assert_eq!(allowed.decision, Decision::Allowed);
    assert_eq!(allowed.matched, vec![0]);

    let implicit = Policy::simulate(&policies, tenant(), Action::CreateGroup, tenant_resource());
    assert_eq!(implicit.decision, Decision::ImplicitDeny);
    assert_eq!(implicit.matched, Vec::<usize>::new());
}

#[test]
fn simulation_can_target_every_workspace_of_the_tenant() {
    let all = Resource::AllWorkspaces {
        tenant_id: tenant(),
    };
    let wildcard = parse(&document("Allow", "entity:GetEntity", "*"), tenant()).unwrap();
    assert_eq!(
        Policy::simulate(&[wildcard], tenant(), Action::GetEntity, all).decision,
        Decision::Allowed
    );
    let one = parse(
        &document("Allow", "entity:GetEntity", &workspace_yrn()),
        tenant(),
    )
    .unwrap();
    assert_eq!(
        Policy::simulate(&[one], tenant(), Action::GetEntity, all).decision,
        Decision::ImplicitDeny
    );
}
