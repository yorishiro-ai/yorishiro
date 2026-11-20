use uuid::Uuid;
use yorishiro::{
    YorishiroError,
    models::iam::trust_policy::{Caller, Principal, TrustPolicy},
};

fn parse(document: &serde_json::Value) -> Result<TrustPolicy, YorishiroError> {
    TrustPolicy::parse(&document.to_string())
}

fn user(id: Uuid) -> serde_json::Value {
    serde_json::json!({"type": "user", "user_id": id})
}

fn caller(user_id: Option<Uuid>, groups: &[Uuid], key: Uuid) -> Caller<'_> {
    Caller {
        user_id,
        group_ids: groups,
        api_key_id: key,
    }
}

#[test]
fn trust_requires_an_allow_that_names_the_caller() {
    let (alice, bob, key) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let policy = parse(
        &serde_json::json!({"statements": [{"effect": "allow", "principals": [user(alice)]}]}),
    )
    .unwrap();
    assert!(policy.permits(&caller(Some(alice), &[], key)));
    assert!(!policy.permits(&caller(Some(bob), &[], key)));
}

#[test]
fn an_explicit_deny_wins_over_any_allow() {
    let (alice, key) = (Uuid::new_v4(), Uuid::new_v4());
    let policy = parse(&serde_json::json!({"statements": [
        {"effect": "allow", "principals": [{"type": "tenant"}]},
        {"effect": "deny", "principals": [user(alice)]},
    ]}))
    .unwrap();
    assert!(!policy.permits(&caller(Some(alice), &[], key)));
    assert!(policy.permits(&caller(Some(Uuid::new_v4()), &[], key)));
}

#[test]
fn a_group_principal_matches_its_members_only() {
    let (group, key) = (Uuid::new_v4(), Uuid::new_v4());
    let policy = parse(&serde_json::json!({"statements": [
        {"effect": "allow", "principals": [{"type": "group", "group_id": group}]},
    ]}))
    .unwrap();
    let member = Uuid::new_v4();
    assert!(policy.permits(&caller(Some(member), &[group], key)));
    assert!(!policy.permits(&caller(Some(member), &[Uuid::new_v4()], key)));
}

#[test]
fn a_service_key_must_be_named_explicitly() {
    let (key, other) = (Uuid::new_v4(), Uuid::new_v4());
    let named = parse(&serde_json::json!({"statements": [
        {"effect": "allow", "principals": [{"type": "service_key", "api_key_id": key}]},
    ]}))
    .unwrap();
    assert!(named.permits(&caller(None, &[], key)));
    assert!(named.names_service_key(key));
    assert!(!named.permits(&caller(None, &[], other)));
    assert!(!named.permits(&caller(Some(Uuid::new_v4()), &[], key)));

    let tenant = parse(&serde_json::json!({"statements": [
        {"effect": "allow", "principals": [{"type": "tenant"}]},
    ]}))
    .unwrap();
    assert!(tenant.permits(&caller(None, &[], key)));
    assert!(!tenant.names_service_key(key));
}

#[test]
fn principals_lists_every_named_principal() {
    let (alice, group) = (Uuid::new_v4(), Uuid::new_v4());
    let policy = parse(&serde_json::json!({"statements": [
        {"effect": "allow", "principals": [user(alice), {"type": "group", "group_id": group}]},
        {"effect": "deny", "principals": [{"type": "tenant"}]},
    ]}))
    .unwrap();
    assert_eq!(
        policy.principals(),
        vec![
            Principal::User { user_id: alice },
            Principal::Group { group_id: group },
            Principal::Tenant
        ]
    );
}

#[test]
fn malformed_documents_are_rejected() {
    let invalid = [
        serde_json::json!({}),
        serde_json::json!({"statements": []}),
        serde_json::json!({"statements": [{"effect": "allow", "principals": []}]}),
        serde_json::json!({"statements": [{"effect": "maybe", "principals": [{"type": "tenant"}]}]}),
        serde_json::json!({"statements": [{"effect": "allow", "principals": [{"type": "everyone"}]}]}),
        serde_json::json!({"statements": [{"effect": "allow", "principals": [{"type": "tenant"}], "actions": ["*"]}]}),
    ];
    for document in invalid {
        assert!(
            matches!(
                parse(&document),
                Err(YorishiroError::ValidationFailed { .. })
            ),
            "{document}"
        );
    }
    let too_many = serde_json::json!({"statements": (0..17)
        .map(|_| serde_json::json!({"effect": "allow", "principals": [{"type": "tenant"}]}))
        .collect::<Vec<_>>()});
    assert!(parse(&too_many).is_err());
    assert!(TrustPolicy::parse(&"x".repeat(20_000)).is_err());
}
