use loco_rs::cli;
use migration::Migrator;
use yorishiro::app::App;

/// A self-hosted deployment is single-tenant unless it says otherwise, which is also what enables the first-run setup wizard.
/// `YORISHIRO_MAX_TENANTS` unset would otherwise resolve to `Ok(None)`, which `setup::wizard_enabled` reads as "no cap, so no wizard", leaving a fresh install with no way to create its first tenant through the product.
/// An operator who wants more than one tenant sets the variable, and that setting is honoured unchanged; `0` still means unlimited.
///
/// Not `#[tokio::main]`: `std::env::set_var` is unsound under concurrent environment access, so the write has to happen before any other thread exists.
/// Building the runtime by hand after the prologue is what guarantees that, and is why this binary does not use the attribute its `ee/` counterpart does.
///
/// Defaulting to a cap rather than to unlimited is a measurement, not a preference: `0` does not
/// mean "unlimited, wizard still available". `tenancy::max_tenants_from_env` folds `0` to
/// `Ok(None)`, the identical value it returns when the variable is unset, and
/// `setup::wizard_enabled` requires `Ok(Some(_))`. So "no cap" and "no wizard" are the same state,
/// and there is no third value expressing unlimited-with-wizard. Defaulting to unlimited here would
/// therefore silently remove the first-run setup wizard from every self-hosted install that never
/// sets the variable.
/// A hosted deployment wanting more than one tenant sets `YORISHIRO_MAX_TENANTS` explicitly, which
/// is cheap for an operator who is provisioning the deployment anyway.
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
