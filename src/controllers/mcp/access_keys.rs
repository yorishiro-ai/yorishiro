//! MCP tools for creating, listing, updating and deleting access keys.

use crate::controllers::mcp::{render, verified};
use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{YorishiroMcpServer, err_to_tool_result, ok_json};
use crate::controllers::access_keys::record;
use crate::controllers::access_keys::render_grant;
use crate::dtos::access_keys::{
    CreateAccessKeyRequest, CreateAccessKeyResponse, ListAccessKeysResponse, UpdateAccessKeyRequest,
};
use crate::models::access_keys::IdentityAccessKeys;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteAccessKeyArgs {
    pub access_key_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateAccessKeyArgs {
    pub access_key_id: Uuid,
    pub name: Option<String>,
    /// The new expiry; applied only with `replace_expires_at`, and omitted to clear the expiry.
    pub expires_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    #[serde(default)]
    pub replace_expires_at: bool,
    /// `false` disables the key so it no longer authenticates; `true` enables it again.
    pub enabled: Option<bool>,
}

#[tool_router(vis = "pub(crate)", router = tool_router_access_keys)]
impl YorishiroMcpServer {
    #[tool(
        description = "Issue an access key and return its secret once. The key acts for the issuing user. Requires iam:CreateAccessKey. The new key cannot exceed the issuer's effective scope. An expiring issuer must supply expires_at no later than its own expiration, except for a root sign-in session."
    )]
    pub(crate) async fn create_access_key(
        &self,
        Parameters(args): Parameters<CreateAccessKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let created = IdentityAccessKeys::create_delegated_access_key(
            &self.app_context().db,
            &auth,
            &args.name,
            args.expires_at,
        )
        .await;
        match created {
            Ok(created) => ok_json(CreateAccessKeyResponse {
                id: created.id,
                prefix: created.key_prefix,
                full_key: created.plaintext,
                name: created.name,
                created_at: crate::data::time::render(self.app_context(), &created.created_at),
                grant: render_grant(self.app_context(), created.grant),
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        description = "List current-tenant access key history including revoked and expired grants without secrets (requires the iam:ListAccessKeys action)"
    )]
    pub(crate) async fn list_access_keys(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let ctx = self.app_context();
        let result = async {
            let records = IdentityAccessKeys::list_with_grants_as(&ctx.db, &auth)
                .await?
                .into_iter()
                .map(|(key, grant)| record(ctx, key, grant))
                .collect();
            Ok::<_, crate::error::YorishiroError>(ListAccessKeysResponse { keys: records })
        }
        .await;
        render(result)
    }

    #[tool(
        description = "Change a current-tenant access key's name, expiry or enabled flag; a disabled key no longer authenticates (requires the iam:UpdateAccessKey action). Set replace_expires_at to apply expires_at, and omit expires_at to clear the expiry"
    )]
    pub(crate) async fn update_access_key(
        &self,
        Parameters(args): Parameters<UpdateAccessKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        let ctx = self.app_context();
        let change = UpdateAccessKeyRequest {
            name: args.name,
            expires_at: args.expires_at,
            replace_expires_at: args.replace_expires_at,
            enabled: args.enabled,
        }
        .into_change();
        let result = IdentityAccessKeys::update_as(&ctx.db, &auth, args.access_key_id, change)
            .await
            .map(|(key, grant)| record(ctx, key, grant));
        render(result)
    }

    #[tool(
        description = "Delete a current-tenant access key so it can no longer authenticate, while retaining secret-free history (requires the iam:DeleteAccessKey action)"
    )]
    pub(crate) async fn delete_access_key(
        &self,
        Parameters(args): Parameters<DeleteAccessKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(self, parts);
        match IdentityAccessKeys::delete_as(&self.app_context().db, &auth, args.access_key_id).await
        {
            Ok(()) => ok_json(serde_json::json!({"deleted":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
