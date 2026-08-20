use super::*;

/// `resolve_embedding_stamp` duplicates `yorishiro_server::embedding_model_name()` rather than calling it, because this crate's lib must not depend on the server crate.
/// This pins the duplication: the stamp is what tells a workspace whose model changed apart from one provisioned under a different one, so a default that drifts from the server's makes that comparison meaningless.
///
/// The first pass of the v0.42.0 lockstep shipped `"local"` here where the server resolves `"multilingual-e5-large"`, so this is a regression test, not a hypothetical.
#[test]
fn the_stamp_matches_what_the_server_would_resolve() {
    assert_eq!(
        resolve_embedding_stamp(None, None, None),
        ("multilingual-e5-large".to_string(), 1024),
        "the local default must match yorishiro_server::embedding_model_name()"
    );
    assert_eq!(
        resolve_embedding_stamp(None, Some("local"), None).0,
        "multilingual-e5-large"
    );
    assert_eq!(
        resolve_embedding_stamp(None, Some("openai"), None).0,
        "openai",
        "an openai provider with no model named falls back to the provider name, as the server does"
    );
    assert_eq!(
        resolve_embedding_stamp(Some("text-embedding-3-small"), Some("openai"), None).0,
        "text-embedding-3-small",
        "an explicit model always wins"
    );
    assert_eq!(resolve_embedding_stamp(None, None, Some("768")).1, 768);
    assert_eq!(
        resolve_embedding_stamp(None, None, Some("not-a-number")).1,
        1024,
        "an unparsable width falls back rather than failing the provisioning"
    );
}
