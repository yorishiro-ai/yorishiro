use yorishiro::services::access_log::path_only;

#[test]
fn access_log_target_excludes_oauth_query_parameters() {
    let uri = "/auth/oauth/callback?code=do-not-log-code&state=do-not-log-state";

    let parsed = uri.parse().expect("test URI must parse");
    let path = path_only(&parsed);

    assert_eq!(path, "/auth/oauth/callback");
    assert!(!path.contains("do-not-log-code"));
    assert!(!path.contains("do-not-log-state"));
}
