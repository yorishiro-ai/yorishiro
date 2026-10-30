use axum::http::request::Parts;
use rmcp::ErrorData;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use super::{VerifyOutcome, YorishiroMcpServer, err_to_tool_result, ok_json};
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
        description = "Issue a workspace-bound API key and return its secret once; cannot exceed issuer scope, audit grant or expiry (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn create_api_key(
        &self,
        Parameters(args): Parameters<CreateApiKeyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("create_api_key")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match IdentityApiKeys::create_delegated_api_key(
            &self.app_context().db,
            &auth,
            args.scope.unwrap_or(ApiKeyScope::Read),
            args.audit,
            &args.name,
            args.expires_at,
        )
        .await
        {
            Ok(created) => ok_json(CreateApiKeyResponse {
                id: created.id,
                prefix: created.key_prefix,
                full_key: created.plaintext,
                name: created.name,
                created_at: crate::time::render(self.app_context(), created.created_at),
                grant: render_grant(self.app_context(), created.grant),
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "List current-tenant API key history including revoked and expired grants without secrets (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn list_api_keys(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("list_api_keys")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        let ctx = self.app_context();
        let result = async {
            crate::models::access::require_tenant_admin(&ctx.db, auth.tenant_id, auth.user_id)
                .await?;
            let mut records = Vec::new();
            for key in IdentityApiKeys::list_for_tenant(&ctx.db, auth.tenant_id).await? {
                let grant = key.grant_metadata(&ctx.db).await?;
                records.push(ApiKeyRecord {
                    id: key.id,
                    prefix: key.key_prefix,
                    name: key.name,
                    created_at: crate::time::render(ctx, key.created_at),
                    last_used_at: key.last_used_at.map(|time| crate::time::render(ctx, time)),
                    grant: render_grant(ctx, grant),
                });
            }
            Ok::<_, crate::error::YorishiroError>(ListApiKeysResponse { keys: records })
        }
        .await;
        match result {
            Ok(records) => ok_json(records),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Revoke a current-tenant API key while retaining secret-free history (requires read scope and tenant Admin or Owner)"
    )]
    pub(crate) async fn revoke_api_key(
        &self,
        Parameters(args): Parameters<ApiKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("revoke_api_key")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match IdentityApiKeys::revoke_for_tenant(&self.app_context().db, &auth, args.key_id).await {
            Ok(()) => ok_json(serde_json::json!({"revoked":true})),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }

    #[tool(
        meta = crate::controllers::mcp::scope_requirement(ApiKeyScope::Read),
        description = "Atomically rotate a current-tenant API key, revoke its predecessor and return the successor secret once (requires read scope and tenant Admin or Owner; issuer ceilings apply)"
    )]
    pub(crate) async fn rotate_api_key(
        &self,
        Parameters(args): Parameters<ApiKeyArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let auth = match super::verify(
            self.app_context(),
            &parts,
            self.required_scope("rotate_api_key")?,
        )
        .await?
        {
            VerifyOutcome::Verified(auth) => auth,
            VerifyOutcome::ScopeDenied(denied) => return Ok(denied),
        };
        match IdentityApiKeys::rotate(&self.app_context().db, &auth, args.key_id).await {
            Ok(created) => ok_json(CreateApiKeyResponse {
                id: created.id,
                prefix: created.key_prefix,
                full_key: created.plaintext,
                name: created.name,
                created_at: crate::time::render(self.app_context(), created.created_at),
                grant: render_grant(self.app_context(), created.grant),
            }),
            Err(error) => Ok(err_to_tool_result(error)),
        }
    }
}
