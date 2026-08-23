//! ID token (JWT) verification, per OpenID Connect Core §3.1.3.7.
//! Ported from master's `ee/crates/yorishiro-hosted/src/services/oauth/id_token.rs`.
//!
//! Signature verification uses whichever key in the provider's JWKS matches the token's `kid`
//! header; `iss`/`aud`/`exp` are checked by `jsonwebtoken`'s own `Validation`.

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use yorishiro_core::YorishiroError;

/// The claims this integration actually needs.
/// OIDC providers may include many more; anything else is ignored by `serde`'s default
/// (non-`deny_unknown_fields`) behavior.
#[derive(Debug, Deserialize)]
pub struct IdTokenClaims {
    pub sub: String,
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    pub name: Option<String>,
}

/// Verifies an ID token's signature against the provider's JWKS and validates its standard
/// claims (`iss` must equal `issuer_url`, `aud` must contain `client_id`, `exp` must be in the
/// future).
/// Returns `YorishiroError::Unauthenticated` for every failure mode (bad signature, wrong
/// issuer/audience, expired, unknown `kid`, unsupported `alg`): a caller has no actionable way
/// to distinguish them, and doing so would only help an attacker fingerprint which check failed.
pub fn verify(
    id_token: &str,
    jwks: &JwkSet,
    issuer_url: &str,
    client_id: &str,
) -> Result<IdTokenClaims, YorishiroError> {
    let header = decode_header(id_token).map_err(|err| {
        tracing::warn!(error = %err, "OAuth ID token header could not be parsed");
        YorishiroError::Unauthenticated
    })?;

    let kid = header.kid.as_deref().ok_or_else(|| {
        tracing::warn!("OAuth ID token header is missing 'kid'");
        YorishiroError::Unauthenticated
    })?;

    let jwk = jwks.find(kid).ok_or_else(|| {
        tracing::warn!(kid, "no matching key in provider JWKS for ID token 'kid'");
        YorishiroError::Unauthenticated
    })?;

    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|err| {
        tracing::warn!(error = %err, "provider JWKS key could not be used for decoding");
        YorishiroError::Unauthenticated
    })?;

    // RS256 is effectively universal among OIDC providers (Google, Microsoft Entra ID, Okta,
    // Auth0 all sign with it by default); restricting to it also closes off the classic
    // "alg: none" / algorithm-confusion class of JWT vulnerabilities by construction, rather
    // than by remembering to check `header.alg` separately.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer_url]);
    validation.set_audience(&[client_id]);

    let data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation).map_err(|err| {
        tracing::warn!(error = %err, "OAuth ID token failed verification");
        YorishiroError::Unauthenticated
    })?;

    Ok(data.claims)
}
