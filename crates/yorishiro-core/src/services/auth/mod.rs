use rand::Rng;
use sea_query::Iden;
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod authenticate;
mod authorize;
mod keys;

pub use authenticate::*;
pub use authorize::*;
pub use keys::*;

/// `pub` (rather than `pub(super)`) only so the crate-root integration test in `tests/` can
/// build its own query against this table; `#[doc(hidden)]` keeps it out of the public API docs.
#[doc(hidden)]
#[derive(Iden)]
pub enum ApiKeys {
    Table,
    Id,
    WorkspaceId,
    KeyHash,
    KeyPrefix,
    Scope,
    UserId,
    LastUsedAt,
}

pub(super) const KEY_PREFIX_BYTES: usize = 6;
pub(super) const KEY_SECRET_BYTES: usize = 24;

/// Permission level held by an API key. Declaration order feeds the derived `Ord`, so any
/// code relying on the `Read < Write < Schema` hierarchy (a higher scope subsumes lower ones)
/// depends on this exact ordering. The `serde` representation matches the DB `scope` column
/// ('read'/'write'/'schema'), so REST/MCP adapters don't need a separate mapping.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyScope {
    Read,
    Write,
    Schema,
}

impl ApiKeyScope {
    fn as_db_str(self) -> &'static str {
        match self {
            ApiKeyScope::Read => "read",
            ApiKeyScope::Write => "write",
            ApiKeyScope::Schema => "schema",
        }
    }

    fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(ApiKeyScope::Read),
            "write" => Some(ApiKeyScope::Write),
            "schema" => Some(ApiKeyScope::Schema),
            _ => None,
        }
    }

    /// Whether a key with this scope can perform an operation requiring `required`.
    /// A higher scope subsumes lower ones (a `write` key is also allowed to `read`).
    pub fn satisfies(self, required: ApiKeyScope) -> bool {
        self >= required
    }
}

/// Workspace, tenant, and scope information resolved by API key authentication. Serves as
/// the starting point for both the subsequent RLS context setup
/// (`TenantDb::acquire_for_workspace`) and scope enforcement. An API key is always scoped to
/// exactly one workspace; `tenant_id` (the workspace's owning tenant) is carried alongside it
/// for tenant-level concerns such as billing checks.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub api_key_id: Uuid,
    pub workspace_id: Uuid,
    pub tenant_id: Uuid,
    pub scope: ApiKeyScope,
    /// The human user this key was issued for, if any. `None` for keys not attributed to a
    /// specific person (e.g. pure service/automation keys).
    pub user_id: Option<Uuid>,
}

pub struct CreatedApiKey {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub scope: ApiKeyScope,
    pub user_id: Option<Uuid>,
    /// The raw API key string. Only its hash is stored in the DB, so this return value is
    /// the only place it can ever be obtained. Callers must make sure to surface it to the user.
    pub plaintext: String,
}

/// Lowercase-hex-encodes `bytes`, two characters per byte (e.g. `[0xab, 0x01]` -> `"ab01"`).
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes a lowercase- or uppercase-hex string back into bytes. Returns `None` (rather than
/// panicking) for an odd-length string or one containing a non-hex-digit/non-ASCII character.
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

pub(crate) fn hash_key(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}
