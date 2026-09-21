use loco_rs::{
    controller::middleware::{MiddlewareLayer, logger},
    environment::Environment,
};
use yorishiro::services::access_log::{Middleware, path_only};

#[test]
fn access_log_target_excludes_oauth_query_parameters() {
    let uri = "/auth/oauth/callback?code=do-not-log-code&state=do-not-log-state";

    let parsed = uri.parse().expect("test URI must parse");
    let path = path_only(&parsed);

    assert_eq!(path, "/auth/oauth/callback");
    assert!(!path.contains("do-not-log-code"));
    assert!(!path.contains("do-not-log-state"));
}

#[test]
fn access_log_configuration_preserves_environment() {
    let middleware = Middleware::new(&logger::Config { enable: true }, &Environment::Test);
    let config = middleware
        .config()
        .expect("middleware config must serialize");

    assert_eq!(config["environment"], "test");
}
