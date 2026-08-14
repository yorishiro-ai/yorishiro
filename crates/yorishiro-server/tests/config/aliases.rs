use super::{RENAMES, plan};
use std::collections::HashMap;

/// Drives `plan` from a map rather than the process environment, so these tests neither read nor
/// write process-wide state and cannot race the rest of the suite.
fn planned(env: &[(&str, &str)]) -> Vec<(String, String, String)> {
    let map: HashMap<&str, &str> = env.iter().copied().collect();
    plan(|k| map.get(k).map(|v| v.to_string()), RENAMES)
        .into_iter()
        .map(|(o, n, v)| (o.to_string(), n.to_string(), v))
        .collect()
}

#[test]
fn an_old_name_is_copied_onto_the_new_one() {
    let out = planned(&[("YSR_BIND", "0.0.0.0:9000")]);

    assert_eq!(
        out,
        vec![(
            "YSR_BIND".to_string(),
            "YORISHIRO_BIND".to_string(),
            "0.0.0.0:9000".to_string()
        )]
    );
}

#[test]
fn a_new_name_that_is_already_set_wins() {
    let out = planned(&[("YSR_BIND", "old"), ("YORISHIRO_BIND", "new")]);

    // Nothing to do: the operator has already migrated this one, and overwriting their new value
    // with the stale old one would be the worst possible reading of "compatibility".
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn nothing_happens_when_only_new_names_are_used() {
    let out = planned(&[
        ("YORISHIRO_BIND", "0.0.0.0:8080"),
        ("YORISHIRO_EMBEDDING_PROVIDER", "local"),
    ]);

    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn an_unrelated_variable_is_untouched() {
    let out = planned(&[("DATABASE_URL", "postgres://x"), ("PATH", "/usr/bin")]);

    assert!(out.is_empty(), "{out:?}");
}

/// Both old spellings of the web directory land on the one new name, which is the point of the
/// rename: the pair was documented twice and configured separately for one behaviour.
#[test]
fn both_old_web_dir_names_map_onto_one() {
    let from_community = planned(&[("YSR_WEB_DIR", "/srv/web")]);
    let from_hosted = planned(&[("YORISHIRO_HOSTED_WEB_DIR", "/srv/web")]);

    assert_eq!(from_community[0].1, "YORISHIRO_WEB_DIR");
    assert_eq!(from_hosted[0].1, "YORISHIRO_WEB_DIR");
}

#[test]
fn every_rename_actually_changes_the_name_and_lands_on_the_product_prefix() {
    for (old, new) in RENAMES {
        assert_ne!(old, new, "{old} maps to itself");
        assert!(
            new.starts_with("YORISHIRO_"),
            "{new} is not on the product prefix"
        );
        assert!(
            !new.starts_with("YORISHIRO_HOSTED_"),
            "{new} keeps the HOSTED infix, which no longer distinguishes anything"
        );
    }
}

/// Exactly two targets are reached by two old names each: the bind address and the web
/// directory, which each had a `YSR_` spelling and a `YORISHIRO_HOSTED_` one for the same
/// setting. Any *other* collision would be a copy-paste error silently sending one variable's
/// value to another variable's reader.
#[test]
fn only_the_known_pairs_share_a_rename_target() {
    let mut targets: HashMap<&str, Vec<&str>> = HashMap::new();
    for (old, new) in RENAMES {
        targets.entry(new).or_default().push(old);
    }
    let mut shared: Vec<&str> = targets
        .iter()
        .filter(|(_, olds)| olds.len() > 1)
        .map(|(new, _)| *new)
        .collect();
    shared.sort_unstable();

    assert_eq!(shared, vec!["YORISHIRO_BIND", "YORISHIRO_WEB_DIR"]);
}
