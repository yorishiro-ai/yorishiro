//! The one environment key read outside Loco's YAML renderer.
#![expect(
    clippy::disallowed_methods,
    reason = "the single place the application reads the environment directly"
)]

pub(crate) fn config_folder() -> Option<std::ffi::OsString> {
    std::env::var_os("LOCO_CONFIG_FOLDER")
}
