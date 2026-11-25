//! Shared test fixtures for tenant/workspace/owner creation.
//!
//! Uses signup for new tenants.

use axum::http::StatusCode;
use loco_rs::app::AppContext;
use sea_orm::TransactionTrait;
use uuid::Uuid;
use yorishiro::models::_entities::api_keys;
use yorishiro::models::api_keys::ApiKeyScope;
use yorishiro::models::tenancy;

/// Caps the tenant's workspaces.
/// No public operation changes this cap (only a plan does), so this is the one place a test writes it directly.
pub(crate) async fn set_workspace_cap(ctx: &AppContext, tenant_id: Uuid, cap: i32) {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set};
    yorishiro::models::tenant_tenants::ActiveModel {
        id: Set(tenant_id),
        max_workspaces: Set(Some(cap)),
        ..Default::default()
    }
    .update(&ctx.db)
    .await
    .expect("set the workspace cap");
}

/// Disables the tenant through the operator operation the `disable_tenant` task runs.
pub(crate) async fn disable_tenant(ctx: &AppContext, tenant_id: Uuid) {
    yorishiro::models::tenant_tenants::retirement::disable_by_operator(
        &ctx.db, tenant_id, tenant_id,
    )
    .await
    .expect("disable the tenant");
}

pub(crate) struct TestTenant {
    pub tenant_id: Uuid,
    pub workspace_id: Uuid,
    pub owner_id: Uuid,
    pub api_key: String,
}

impl TestTenant {
    #[expect(
        clippy::future_not_send,
        reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
    )]
    pub(crate) async fn create(
        request: &loco_rs::TestServer,
        _ctx: &AppContext,
        args: TenantArgs,
    ) -> Self {
        let response = request
            .post("/auth/signup")
            .json(&serde_json::json!({
                "email": args.owner_email,
                "password": args.owner_password,
                "display_name": args.tenant_name,
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
        let account = response.json::<serde_json::Value>();
        let tenant_id =
            serde_json::from_value(account["tenant_id"].clone()).expect("signup tenant ID");
        let owner_id = serde_json::from_value(account["user_id"].clone()).expect("signup owner ID");
        let login = request
            .post("/auth/root/login")
            .json(&serde_json::json!({
                "email": args.owner_email,
                "password": args.owner_password,
            }))
            .await;
        login.assert_status_ok();
        let login = login.json::<serde_json::Value>();
        let bearer = format!("Bearer {}", login["api_key"].as_str().expect("login key"));
        let workspace = request
            .post("/api/workspaces")
            .add_header("Authorization", &bearer)
            .json(&serde_json::json!({"name": args.workspace_name}))
            .await;
        workspace.assert_status(StatusCode::CREATED);
        let workspace_id: Uuid =
            serde_json::from_value(workspace.json::<serde_json::Value>()["id"].clone())
                .expect("workspace ID");
        let key = request
            .post("/api/api-keys")
            .add_header("Authorization", &bearer)
            .add_header("X-Workspace-Id", workspace_id.to_string())
            .json(&serde_json::json!({
                "name": "test-workspace-key",
                "scope": args.key_scope,
                "audit": args.key_audit,
            }))
            .await;
        key.assert_status(StatusCode::CREATED);
        let api_key = key.json::<serde_json::Value>()["full_key"]
            .as_str()
            .expect("workspace API key")
            .to_owned();
        Self {
            tenant_id,
            workspace_id,
            owner_id,
            api_key,
        }
    }
}

pub(crate) async fn grant_iam_administrator(
    txn: &sea_orm::DatabaseTransaction,
    tenant_id: Uuid,
    user_id: Uuid,
) {
    use sea_orm::{ActiveModelTrait, Set};
    let policy = yorishiro::models::iam_policies::ActiveModel {
        tenant_id: Set(tenant_id),
        name: Set(format!("fixture-admin-{user_id}")),
        document: Set(
            serde_json::json!({"statements":[{"effect":"allow","actions":["*"],
            "resources":[{"type":"tenant","tenant_id":tenant_id},
                {"type":"all_workspaces","tenant_id":tenant_id}]}]}),
        ),
        ..Default::default()
    }
    .insert(txn)
    .await
    .expect("create fixture administrator policy");
    yorishiro::models::iam_user_policy_attachments::ActiveModel {
        tenant_id: Set(tenant_id),
        user_id: Set(user_id),
        policy_id: Set(policy.id),
        ..Default::default()
    }
    .insert(txn)
    .await
    .expect("attach fixture administrator policy");
}

pub(crate) async fn create_iam_administrator(ctx: &AppContext, tenant_id: Uuid) -> Uuid {
    let txn = ctx.db.begin().await.expect("begin administrator setup");
    let user = tenancy::register_member(
        &txn,
        tenant_id,
        &unique_email("administrator"),
        "hunter2-hunter2",
        None,
    )
    .await
    .expect("create administrator");
    grant_iam_administrator(&txn, tenant_id, user.id).await;
    txn.commit().await.expect("commit administrator setup");
    user.id
}

/// Arguments for shared tenant/workspace/owner setup.
pub(crate) struct TenantArgs {
    /// Tenant name. Defaults to a unique name.
    pub tenant_name: String,
    /// Workspace name. Defaults to a unique name.
    pub workspace_name: String,
    /// Owner email. Defaults to a unique address.
    pub owner_email: String,
    /// Owner password. Defaults to `"hunter2-hunter2"`.
    pub owner_password: String,
    /// API key scope. Defaults to `Migration`.
    pub key_scope: ApiKeyScope,
    /// Whether the audit key is audit-scoped.
    pub key_audit: bool,
}

impl Default for TenantArgs {
    fn default() -> Self {
        Self {
            tenant_name: format!("tenant-{}", Uuid::new_v4().simple()),
            workspace_name: format!("workspace-{}", Uuid::new_v4().simple()),
            owner_email: unique_email("owner"),
            owner_password: "hunter2-hunter2".into(),
            key_scope: ApiKeyScope::Migration,
            key_audit: false,
        }
    }
}

/// Issue an API key for an existing user.
///
/// Create a `TestTenant` first, then use its owner ID for additional scoped keys.
pub(crate) async fn issue_api_key(
    ctx: &AppContext,
    workspace_id: Uuid,
    user_id: Uuid,
    scope: ApiKeyScope,
    audit: bool,
) -> String {
    api_keys::Entity::create_api_key(&ctx.db, workspace_id, scope, Some(user_id), audit)
        .await
        .expect("issue api key")
        .plaintext
}

pub(crate) async fn issue_tenant_key(ctx: &AppContext, tenant_id: Uuid, user_id: Uuid) -> String {
    use sea_orm::EntityTrait;
    if yorishiro::models::iam_tenant_roots::Entity::find_by_id(tenant_id)
        .one(&ctx.db)
        .await
        .expect("check test principal")
        .is_some_and(|root| root.user_id == user_id)
    {
        return tenancy::issue_login_key(&ctx.db, user_id, None, Some(tenant_id))
            .await
            .expect("issue root session")
            .0
            .plaintext;
    }
    api_keys::Entity::create_tenant_api_key(
        &ctx.db,
        tenant_id,
        ApiKeyScope::Migration,
        Some(user_id),
        false,
        "test-tenant-admin",
    )
    .await
    .expect("issue tenant key")
    .plaintext
}

/// Installs a licence for `plan` that stays valid for an hour.
#[cfg(feature = "enterprise")]
pub(crate) fn install_licence(ctx: &AppContext, plan: &str) {
    use std::sync::Arc;
    use yorishiro::edition::ee::controllers::middleware::edition::{LicenceClaims, LicenceState};
    ctx.shared_store
        .insert(Arc::new(LicenceState::licensed(LicenceClaims {
            sub: "test".into(),
            plan: plan.into(),
            exp: chrono::Utc::now().timestamp() + 3600,
        })));
}

/// An email address no other test in the same database uses.
pub(crate) fn unique_email(prefix: &str) -> String {
    format!("{prefix}-{}@example.com", Uuid::new_v4().simple())
}

/// A one-field `note` schema definition named `name`; `embedded` marks its title for embedding.
pub(crate) fn note_schema(name: &str, embedded: bool) -> serde_json::Value {
    let mut title = serde_json::json!({"type": "string", "required": true});
    if embedded {
        title["x-embed"] = true.into();
    }
    serde_json::json!({
        "name": name,
        "entity_types": {"note": {"fields": {"title": title}}}
    })
}

/// Waits until a session holds (`granted`) or queues for (`!granted`) the advisory lock named `fence`.
///
/// PostgreSQL only: it reads `pg_locks`.
pub(crate) async fn wait_for_membership_lock(
    db: &sea_orm::DatabaseConnection,
    fence: &str,
    granted: bool,
) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

    super::jobs::wait_until(
        "the membership fence holder or waiter",
        std::time::Duration::from_secs(20),
        || async {
            let row = db
                .query_one_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted = $2 AND classid::bigint = ((hashtextextended($1, 0) >> 32) & 4294967295) AND objid::bigint = (hashtextextended($1, 0) & 4294967295) AND objsubid = 1) AS observed",
                    [fence.into(), granted.into()],
                ))
                .await
                .unwrap()
                .unwrap();
            row.try_get::<bool>("", "observed").unwrap().then_some(())
        },
    )
    .await;
}

/// Creates a workspace under `tenant_id` and commits it.
pub(crate) async fn create_workspace(
    ctx: &AppContext,
    tenant_id: Uuid,
    name: &str,
) -> yorishiro::models::_entities::workspace_workspaces::Model {
    let txn = ctx.db.begin().await.expect("begin workspace creation");
    let workspace = tenancy::create_workspace(&txn, tenant_id, name, None, None)
        .await
        .expect("create workspace");
    txn.commit().await.expect("commit workspace creation");
    workspace
}

/// Creates a workspace named `default` through the REST API and returns its ID.
#[expect(
    clippy::future_not_send,
    reason = "the request harness uses thread-local HTTP futures"
)]
pub(crate) async fn rest_create_workspace(request: &loco_rs::TestServer, api_key: &str) -> Uuid {
    let response = request
        .post("/api/workspaces")
        .add_header("Authorization", format!("Bearer {api_key}"))
        .json(&serde_json::json!({"name": "default"}))
        .await;
    response.assert_status(StatusCode::CREATED);
    response.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// Creates a `note` schema and one entity through the REST API and returns the entity ID.
#[cfg(feature = "enterprise")]
#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
pub(crate) async fn rest_create_entity(request: &loco_rs::TestServer, setup: &TestTenant) -> Uuid {
    let create_schema = request
        .post("/api/schemas")
        .add_header("Authorization", format!("Bearer {}", setup.api_key))
        .json(&serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": {
                    "fields": {
                        "title": { "type": "string", "required": true },
                        "summary": { "type": "string" }
                    }
                }
            }
        }))
        .await;
    assert_eq!(
        create_schema.status_code(),
        201,
        "response: {:?}",
        create_schema.text()
    );

    let create_entity = request
        .post("/api/entities")
        .add_header("Authorization", format!("Bearer {}", setup.api_key))
        .json(&serde_json::json!({
            "schema_name": "note",
            "entity_type": "note",
            "data": { "title": "original" }
        }))
        .await;
    assert_eq!(
        create_entity.status_code(),
        201,
        "response: {:?}",
        create_entity.text()
    );
    let entity_id: Uuid = create_entity.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    entity_id
}
