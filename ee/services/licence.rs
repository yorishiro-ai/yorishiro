//! Licence key verification for the enterprise features under `ee/`.
//!
//! The key is an RS256-signed JWT.
//! The matching public key is compiled into the binary, so verification needs no network and no configuration beyond the key itself (`YORISHIRO_LICENSE_KEY`).
//!
//! **This check is removable.** The verifying code ships in source form, so anyone can delete these lines and rebuild.
//! That is deliberate: the protection is `ee/LICENSE`, which makes using such a build a licence violation, not this function.
//! Do not add obfuscation here under the impression it changes that.
//!
//! No key means the enterprise features are disabled, never that the process refuses to start: a deployment that only wants the free half must keep working with no licence configured at all.

use crate::YorishiroError;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

/// The public half of the signing key, compiled in.
/// Rotating it means replacing this file and cutting a release; keys signed by the previous private key stop verifying at that point.
// Resolved from this file's own directory (`ee/services/`), so the key stays inside `ee/` beside
// the code it verifies for.
const PUBLIC_KEY_PEM: &[u8] = include_bytes!("../keys/licence-public.pem");

/// What a licence key asserts.
///
/// `plan` is recorded and logged but gates nothing yet: every valid, unexpired key unlocks every enterprise feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenceClaims {
    /// Who the licence was issued to.
    /// Free-form, and routinely an email address, so it is deliberately not logged, see `from_env`.
    pub sub: String,
    pub plan: String,
    /// Expiry, as a Unix timestamp.
    /// Checked at verification *and* again at each gate, so a key that lapses while the process runs stops working without a restart.
    pub exp: i64,
}

/// Which of the two sources wins, as a pure function so the precedence is testable without touching the process environment: tests that set a variable race each other.
///
/// The file is consulted only when the variable is **absent**.
/// Set-but-empty means "no licence" rather than falling through, or `YORISHIRO_LICENSE_KEY=` could not turn off a licence configured in the file.
pub(crate) fn resolve_licence_key(
    from_env: Option<String>,
    from_file: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match from_env {
        Some(value) => Some(value).filter(|v| !v.is_empty()),
        None => from_file(),
    }
}

/// `license_key:` from the config file, read here rather than in a shared config loader.
///
/// `config.yml` does not exist in this crate's own config directory, so this always returns `None` and only the environment-variable path (`YORISHIRO_LICENSE_KEY`, checked in `LicenceState::from_env` before this is ever called) is live.
fn licence_key_from_config() -> Option<String> {
    let path = std::env::var("YORISHIRO_CONFIG_PATH").unwrap_or_else(|_| "config.yml".into());
    licence_key_in(&std::fs::read_to_string(path).ok()?)
}

/// The parse [`licence_key_from_config`] wraps, split out so it is testable without a file or the process environment: tests that set `YORISHIRO_CONFIG_PATH` would race each other.
pub fn licence_key_in(yaml: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct JustTheLicence {
        license_key: Option<String>,
    }

    // Not `deny_unknown_fields`: this reads one key out of a file whose other keys belong to a different struct.
    let parsed: JustTheLicence = serde_yaml_ng::from_str(yaml).ok()?;
    parsed.license_key.filter(|k| !k.is_empty())
}

/// Verifies a licence key against a PEM-encoded RSA public key.
///
/// Split from [`LicenceState::from_env`] so tests can verify against their own key rather than the compiled-in one.
/// Failures are logged in detail, unlike OAuth token failures: the caller here is an operator debugging a key they hold, not an untrusted client whose probing should not be helped along.
pub fn verify(token: &str, public_key_pem: &[u8]) -> Result<LicenceClaims, YorishiroError> {
    let key = DecodingKey::from_rsa_pem(public_key_pem).map_err(|err| {
        YorishiroError::Internal(anyhow::anyhow!(
            "licence public key is not a usable RSA PEM: {err}"
        ))
    })?;

    // RS256 only.
    // Naming one algorithm is what rules out the "alg: none" and algorithm-confusion families by construction rather than by remembering to check the header separately.
    let mut validation = Validation::new(Algorithm::RS256);
    // The key is issued by us for us; there is no issuer or audience to distinguish.
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    validation.set_required_spec_claims(&["exp"]);

    let data = decode::<LicenceClaims>(token, &key, &validation).map_err(|err| {
        tracing::warn!(error = %err, "licence key failed verification");
        YorishiroError::Unauthenticated
    })?;

    Ok(data.claims)
}

/// The licence a running process holds, resolved once at startup.
///
/// Verification happens at startup so a malformed key is reported then rather than on the first request that needs it.
/// Expiry is *not* frozen at startup: [`Self::is_active`] compares against the current time, so a long-running process stops serving enterprise features when the key lapses.
#[derive(Debug, Clone, Default)]
pub struct LicenceState {
    claims: Option<LicenceClaims>,
}

impl LicenceState {
    /// Reads `YORISHIRO_LICENSE_KEY` and verifies it against the compiled-in public key.
    ///
    /// An absent, empty or invalid key all yield an unlicensed state rather than aborting startup: refusing to boot would take down the free half over a enterprise-feature misconfiguration.
    /// An invalid one is logged at `warn`, since it almost certainly means someone expected enterprise features to be on.
    pub fn from_env() -> Self {
        let from_env =
            std::env::var_os("YORISHIRO_LICENSE_KEY").map(|v| v.into_string().unwrap_or_default());

        let Some(token) = resolve_licence_key(from_env, licence_key_from_config) else {
            tracing::info!("no licence key configured: enterprise features are disabled");
            return Self::default();
        };

        match verify(&token, PUBLIC_KEY_PEM) {
            Ok(claims) => {
                // `sub` is free-form and routinely an email address, so it does not go in a routine log line.
                // Plan and expiry are what an operator needs to see; the issuee is in the key they already hold.
                tracing::info!(
                    plan = %claims.plan,
                    expires_at = claims.exp,
                    "licence key accepted: enterprise features are enabled"
                );
                Self {
                    claims: Some(claims),
                }
            }
            Err(_) => {
                tracing::warn!(
                    "licence key was set but did not verify: enterprise features are disabled"
                );
                Self::default()
            }
        }
    }

    /// Builds a state directly from claims, for tests and for a caller that verified elsewhere.
    pub fn licensed(claims: LicenceClaims) -> Self {
        Self {
            claims: Some(claims),
        }
    }

    /// Whether enterprise features are currently unlocked: a verified key that has not yet expired.
    pub fn is_active(&self) -> bool {
        self.is_active_at(chrono::Utc::now().timestamp())
    }

    /// The pure fold [`Self::is_active`] wraps, so expiry is testable without waiting for a clock or mocking one.
    pub fn is_active_at(&self, now: i64) -> bool {
        self.claims.as_ref().is_some_and(|c| c.exp > now)
    }
}
