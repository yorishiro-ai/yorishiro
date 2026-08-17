//! Licence key verification for the paid features under `ee/`.
//!
//! The key is an RS256-signed JWT. The matching public key is compiled into the binary, so
//! verification needs no network and no configuration beyond the key itself
//! (`YORISHIRO_LICENSE_KEY`).
//!
//! **This check is removable.** The verifying code ships in source form, so anyone can delete
//! these lines and rebuild. That is deliberate: the protection is `ee/LICENSE`, which makes
//! using such a build a licence violation, not this function. Do not add obfuscation here under the impression it changes that.
//!
//! No key means the paid features are disabled, never that the process refuses to start: a
//! deployment that only wants the free half must keep working with no licence configured at all.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};
use yorishiro_core::YorishiroError;

/// The public half of the signing key, compiled in. Rotating it means replacing this file and
/// cutting a release; keys signed by the previous private key stop verifying at that point.
const PUBLIC_KEY_PEM: &[u8] = include_bytes!("../../keys/licence-public.pem");

/// What a licence key asserts.
///
/// `plan` is recorded and logged but gates nothing yet: every valid, unexpired key unlocks every
/// paid feature. A plan-to-feature matrix is not provided: there is one plan to sell, and a
/// mapping built before the second one exists would encode a guess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenceClaims {
    /// Who the licence was issued to. Free-form, and routinely an email address, so it is
    /// deliberately not logged, see `from_env`.
    pub sub: String,
    pub plan: String,
    /// Expiry, as a Unix timestamp. Checked at verification *and* again at each gate, so a key
    /// that lapses while the process runs stops working without a restart.
    pub exp: i64,
}

/// Which of the two sources wins, as a pure function so the precedence is testable without
/// touching the process environment: tests that set a variable race each other.
///
/// The file is consulted only when the variable is **absent**. Set-but-empty means the
/// environment has spoken and the answer is "no licence", matching every other setting: the
/// shared loader skips the file whenever the variable exists at all. Without that,
/// `YORISHIRO_LICENSE_KEY=` could not turn off a licence configured in the file, despite the
/// environment being what takes precedence.
pub(crate) fn resolve_licence_key(
    from_env: Option<String>,
    from_file: impl FnOnce() -> Option<String>,
) -> Option<String> {
    match from_env {
        Some(value) => Some(value).filter(|v| !v.is_empty()),
        None => from_file(),
    }
}

/// `license_key:` from the config file, read here rather than in `yorishiro-server`'s shared
/// loader.
///
/// That loader copies every setting it parses into the environment, and doing so for this one
/// would put the string `YORISHIRO_LICENSE_KEY` into the community binary, which the release
/// gate scans for and rejects, correctly: that build is meant to carry no trace of the paid
/// edition. The shared struct therefore accepts the key and ignores it, and the edition that
/// actually uses it reads the file itself.
///
/// Environment first, file second, matching every other setting.
fn licence_key_from_config() -> Option<String> {
    let path = std::env::var("YORISHIRO_CONFIG_PATH").unwrap_or_else(|_| "config.yml".into());
    licence_key_in(&std::fs::read_to_string(path).ok()?)
}

/// The parse [`licence_key_from_config`] wraps, split out so it is testable without a file or
/// the process environment: tests that set `YORISHIRO_CONFIG_PATH` would race each other.
pub fn licence_key_in(yaml: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct JustTheLicence {
        license_key: Option<String>,
    }

    // Not `deny_unknown_fields`: this reads one key out of a file whose other keys belong to a
    // different struct, so everything else has to pass through rather than be rejected.
    let parsed: JustTheLicence = serde_yaml_ng::from_str(yaml).ok()?;
    parsed.license_key.filter(|k| !k.is_empty())
}

/// Verifies a licence key against a PEM-encoded RSA public key.
///
/// Split from [`LicenceState::from_env`] so tests can verify against their own key rather than
/// the compiled-in one. Failures are logged in detail, unlike OAuth token failures: the caller
/// here is an operator debugging a key they hold, not an untrusted client whose probing should
/// not be helped along.
pub fn verify(token: &str, public_key_pem: &[u8]) -> Result<LicenceClaims, YorishiroError> {
    let key = DecodingKey::from_rsa_pem(public_key_pem).map_err(|err| {
        YorishiroError::Internal(anyhow::anyhow!(
            "licence public key is not a usable RSA PEM: {err}"
        ))
    })?;

    // RS256 only. Naming one algorithm is what rules out the "alg: none" and algorithm-confusion
    // families by construction rather than by remembering to check the header separately.
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
/// Verification happens at startup so a malformed key is reported then rather than on the first
/// request that needs it. Expiry is *not* frozen at startup: [`Self::is_active`] compares
/// against the current time, so a long-running process stops serving paid features when the key
/// lapses.
#[derive(Debug, Clone, Default)]
pub struct LicenceState {
    claims: Option<LicenceClaims>,
}

impl LicenceState {
    /// Reads `YORISHIRO_LICENSE_KEY` and verifies it against the compiled-in public key.
    ///
    /// An absent or empty variable yields an unlicensed state, which is a supported way to run:
    /// the free half works and the paid gates answer 404. A *present but invalid* key also
    /// yields an unlicensed state rather than aborting startup, because refusing to boot would
    /// take down the free half over a paid-feature misconfiguration, but it is logged at
    /// `warn`, since it almost certainly means someone expected paid features to be on.
    pub fn from_env() -> Self {
        let from_env =
            std::env::var_os("YORISHIRO_LICENSE_KEY").map(|v| v.into_string().unwrap_or_default());

        let Some(token) = resolve_licence_key(from_env, licence_key_from_config) else {
            tracing::info!("no licence key configured: paid features are disabled");
            return Self::default();
        };

        match verify(&token, PUBLIC_KEY_PEM) {
            Ok(claims) => {
                // `sub` is free-form and routinely an email address, so it does not go in a
                // routine log line. Plan and expiry are what an operator needs to see; the
                // issuee is in the key they already hold.
                tracing::info!(
                    plan = %claims.plan,
                    expires_at = claims.exp,
                    "licence key accepted: paid features are enabled"
                );
                Self {
                    claims: Some(claims),
                }
            }
            Err(_) => {
                tracing::warn!(
                    "licence key was set but did not verify: paid features are disabled"
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

    /// Whether paid features are currently unlocked: a verified key that has not yet expired.
    pub fn is_active(&self) -> bool {
        self.is_active_at(chrono::Utc::now().timestamp())
    }

    /// The pure fold [`Self::is_active`] wraps, so expiry is testable without waiting for a clock
    /// or mocking one.
    pub fn is_active_at(&self, now: i64) -> bool {
        self.claims.as_ref().is_some_and(|c| c.exp > now)
    }

    /// The error a gated endpoint returns when no active licence is held.
    ///
    /// 404 rather than 402 or 403, matching the setup wizard's answer for a capability this
    /// deployment does not offer: the endpoint is genuinely not being served here. The message
    /// names the reason, because the operator is the one who can fix it.
    pub fn require_active(&self) -> Result<(), YorishiroError> {
        if self.is_active() {
            return Ok(());
        }
        Err(YorishiroError::not_found(
            "this feature requires a licence key (set YORISHIRO_LICENSE_KEY)",
        ))
    }
}

#[cfg(test)]
#[path = "../../tests/services/licence.rs"]
mod tests;
