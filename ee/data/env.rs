//! Direct environment keys owned by the enterprise overlay.

use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Env {
    LicenceKey,
}

impl Env {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::LicenceKey => "YORISHIRO_LICENSE_KEY",
        }
    }

    pub(crate) fn read_os(self) -> Option<OsString> {
        std::env::var_os(self.name())
    }
}
