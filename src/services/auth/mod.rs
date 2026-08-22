use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod authenticate;
mod authenticator;
mod authorize;

pub use authenticate::*;
pub use authenticator::*;
pub use authorize::*;

pub(crate) const KEY_PREFIX_BYTES: usize = 6;
pub(crate) const KEY_SECRET_BYTES: usize = 24;

/// Permission level held by an API key.
/// Declaration order feeds the derived `Ord`: `Read < Write < Schema < Migration`, a higher
/// scope subsumes lower ones. The serde representation matches the DB `scope` column
/// ('read'/'write'/'schema'/'migration').
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    Read,
    Write,
    Schema,
    /// Running a batch migration, and switching maintenance mode. Above `schema` because both
    /// act on data already stored.
    Migration,
}

impl ApiKeyScope {
    pub fn as_db_str(self) -> &'static str {
        match self {
            ApiKeyScope::Read => "read",
            ApiKeyScope::Write => "write",
            ApiKeyScope::Schema => "schema",
            ApiKeyScope::Migration => "migration",
        }
    }

    /// `None` for anything this crate does not define, which the caller should treat as a
    /// corrupt row rather than a missing scope.
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ApiKeyScope::Read),
            "write" => Some(ApiKeyScope::Write),
            "schema" => Some(ApiKeyScope::Schema),
            "migration" => Some(ApiKeyScope::Migration),
            _ => None,
        }
    }

    /// Whether a key with this scope can perform an operation requiring `required`.
    pub fn satisfies(self, required: ApiKeyScope) -> bool {
        self >= required
    }
}

/// Workspace, tenant, and scope information resolved by API key authentication.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The human user this key was issued for, if any.
    pub user_id: Option<Uuid>,
}

pub struct CreatedApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Option<Uuid>,
    /// The raw API key string. Only its hash is stored in the DB, so this return value is the
    /// only place it can ever be obtained.
    pub plaintext: String,
}

/// Extracts the API key from an `Authorization` header value, or `None` if the header is
/// absent, is not a `Bearer` credential, or carries an empty one.
///
/// Every adapter that authenticates a request routes through here, so `Authorization: Bearer `
/// with nothing after it gets the same answer everywhere.
pub fn bearer_credential(header_value: Option<&str>) -> Option<&str> {
    header_value
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
}

/// Lowercase-hex-encodes `bytes`, two characters per byte.
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes a lowercase- or uppercase-hex string back into bytes.
/// Returns `None` for an odd-length string or one containing a non-hex-digit character.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes = s.as_bytes();
    (0..bytes.len())
        .step_by(2)
        .map(|i| {
            let pair = std::str::from_utf8(&bytes[i..i + 2]).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

pub(crate) fn random_hex(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::rng().fill_bytes(&mut bytes);
    hex_encode(&bytes)
}

/// Hashes a presented key into the form stored in `identity_api_keys.key_hash`.
pub fn hash_key(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}
