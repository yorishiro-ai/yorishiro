//! The one environment key read outside Loco's YAML renderer.

pub(crate) fn config_folder() -> Option<std::ffi::OsString> {
    std::env::var_os("LOCO_CONFIG_FOLDER")
}
