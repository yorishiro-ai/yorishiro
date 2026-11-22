//! MCP tools for creating, listing, rotating and revoking API keys.

use crate::controllers::mcp::{render, try_tool, verified};
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
use crate::controllers::api_keys::render_grant;
use crate::dtos::api_keys::{
    ApiKeyRecord, CreateApiKeyRequest, CreateApiKeyResponse, ListApiKeysResponse,
};
use crate::models::api_keys::{ApiKeyScope, IdentityApiKeys};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiKeyArgs {
    pub key_id: Uuid,
}

#[tool_router(vis = "pub(crate)", router = tool_router_api_keys)]
impl YorishiroMcpServer {
    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Issue an API key and return its secret once. Select a workspace to bind the key, or set tenant_scoped without a workspace selector for an unbound key. Requires a tenant-scoped issuer with read scope and credential_create permission. The new key cannot exceed the issuer's effective scope or audit grant. An expiring issuer must supply expires_at no later than its own expiration, except for a root sign-in session."
    )]
    pub(crate) async fn create_api_key(
        &self,
        Parameters(args): Parameters<CreateApiKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(
            super::verify(
                self.app_context(),
                &parts,
                self.required_scope("create_api_key")?,
            )
            .await?
        );
        try_tool!(
            args.validate_workspace_selection(
                parts
                    .headers
                    .contains_key(crate::models::api_keys::WORKSPACE_HEADER),
            )
        );
        let created = if args.tenant_scoped {
            IdentityApiKeys::create_delegated_tenant_api_key(
                &self.app_context().db,
                &auth,
                args.scope.unwrap_or(ApiKeyScope::Read),
                args.audit,
                &args.name,
                args.expires_at,
            )
            .await
        } else {
            IdentityApiKeys::create_delegated_api_key(
                &self.app_context().db,
                &auth,
                args.scope.unwrap_or(ApiKeyScope::Read),
                args.audit,
                &args.name,
                args.expires_at,
            )
            .await
        };
        match created {
            Ok(created) => ok_json(CreateApiKeyResponse {
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
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List current-tenant API key history including revoked and expired grants without secrets (requires read scope and credential_list IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn list_api_keys(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(
            super::verify(
                self.app_context(),
                &parts,
                self.required_scope("list_api_keys")?,
            )
            .await?
        );
        let ctx = self.app_context();
        let result = async {
            let mut records = Vec::new();
            for (key, grant) in IdentityApiKeys::list_with_grants_as(&ctx.db, &auth).await? {
                records.push(ApiKeyRecord {
                    id: key.id,
                    prefix: key.key_prefix,
                    name: key.name,
                    created_at: crate::data::time::render(ctx, &key.created_at),
                    last_used_at: key
                        .last_used_at
                        .map(|time| crate::data::time::render(ctx, &time)),
                    grant: render_grant(ctx, grant),
                });
            }
            Ok::<_, crate::error::YorishiroError>(ListApiKeysResponse { keys: records })
        }
        .await;
        render(result)
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Revoke a current-tenant API key while retaining secret-free history (requires read scope and credential_revoke IAM permission with a tenant-scoped credential)"
    )]
    pub(crate) async fn revoke_api_key(
        &self,
        Parameters(args): Parameters<ApiKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(
            super::verify(
                self.app_context(),
                &parts,
                self.required_scope("revoke_api_key")?,
            )
            .await?
        );
        match IdentityApiKeys::revoke_for_tenant(&self.app_context().db, &auth, args.key_id).await {
            Ok(()) => ok_json(serde_json::json!({"revoked":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Atomically rotate a current-tenant API key, revoke its predecessor and return the successor secret once (requires read scope and credential_rotate IAM permission with a tenant-scoped credential; issuer ceilings apply)"
    )]
    pub(crate) async fn rotate_api_key(
        &self,
        Parameters(args): Parameters<ApiKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = verified!(
            super::verify(
                self.app_context(),
                &parts,
                self.required_scope("rotate_api_key")?,
            )
            .await?
        );
        match IdentityApiKeys::rotate(&self.app_context().db, &auth, args.key_id).await {
            Ok(created) => ok_json(CreateApiKeyResponse {
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
}
