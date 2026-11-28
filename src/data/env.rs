//! The one environment key read outside Loco's YAML renderer.
#![expect(
    clippy::disallowed_methods,
    reason = "the single place the application reads the environment directly"
)]

pub(crate) fn config_folder() -> Option<std::ffi::OsString> {
    std::env::var_os("LOCO_CONFIG_FOLDER")
}

/// The operating-system user and host that started a command, for the trail.
pub(crate) fn operator() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
    let host = std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map_or_else(|| "unknown".to_owned(), |host| host.trim().to_owned());
    format!("{user}@{host}")
}
