use loco_rs::cli;
use migration::Migrator;
use yorishiro_core::app::App;

/// A self-hosted deployment is single-tenant unless it says otherwise, which is also what enables the first-run setup wizard.
/// `YORISHIRO_MAX_TENANTS` unset would otherwise resolve to `Ok(None)`, which `setup::wizard_enabled` reads as "no cap, so no wizard", leaving a fresh install with no way to create its first tenant through the product.
/// An operator who wants more than one tenant sets the variable, and that setting is honoured unchanged; `0` still means unlimited.
///
/// Not `#[tokio::main]`: `std::env::set_var` is unsound under concurrent environment access, so the write has to happen before any other thread exists.
/// Building the runtime by hand after the prologue is what guarantees that, and is why this binary does not use the attribute its `ee/` counterpart does.
///
/// `ee/`'s binary deliberately has no such prologue: the paid edition is the multi-tenant product, so unlimited is the right default there.
fn main() -> loco_rs::Result<()> {
    // SAFETY: no other thread exists at this point in `main`; the tokio runtime is built below.
    unsafe {
        if std::env::var_os("YORISHIRO_MAX_TENANTS").is_none() {
            std::env::set_var("YORISHIRO_MAX_TENANTS", "1");
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(loco_rs::Error::IO)?
        .block_on(cli::main::<App, Migrator>())
}
