use chrono::Utc;
use loco_rs::testing::prelude::*;
use sea_orm::{ConnectionTrait, Statement};
use serial_test::serial;
use uuid::Uuid;
use yorishiro_core::db::DbHandle;
use yorishiro_core::models::_entities::{identity_api_keys, identity_tenants, identity_workspaces};
use yorishiro_core::models::identity_workspaces::WORKSPACE_STATUS_ACTIVE;
use yorishiro_core::models::tenancy::{self, MembershipRole};
use yorishiro_core::services::auth::ApiKeyScope;
use yorishiro_hosted::HostedApp;
use yorishiro_hosted::models::fill_proposals;
use yorishiro_hosted::services::licence::{LicenceClaims, LicenceState};

/// `shared_store.insert` is keyed by `TypeId`, so this overwrites the `LicenceState::from_env()` the test process booted with.
/// See `marketplace.rs`'s own copy of this helper.
fn licence(ctx: &loco_rs::app::AppContext) {
    ctx.shared_store
        .insert(LicenceState::licensed(LicenceClaims {
            sub: "acme-corp".into(),
            plan: "enterprise".into(),
            exp: Utc::now().timestamp() + 60 * 60,
        }));
}

struct Setup {
    key: String,
    tenant_id: Uuid,
    workspace_id: Uuid,
}

async fn setup(ctx: &loco_rs::app::AppContext) -> Setup {
    let tenant = identity_tenants::ActiveModel {
        name: sea_orm::ActiveValue::Set("acme".into()),
        ..Default::default()
    };
    let tenant = sea_orm::ActiveModelTrait::insert(tenant, &ctx.db)
        .await
        .expect("insert tenant");
    let workspace = identity_workspaces::ActiveModel {
        tenant_id: sea_orm::ActiveValue::Set(tenant.id),
        name: sea_orm::ActiveValue::Set("main".into()),
        status: sea_orm::ActiveValue::Set(WORKSPACE_STATUS_ACTIVE.to_string()),
        ..Default::default()
    };
    let workspace = sea_orm::ActiveModelTrait::insert(workspace, &ctx.db)
        .await
        .expect("insert workspace");
    let owner = tenancy::create_user(&ctx.db, "owner@example.com", "hunter2-hunter2", None)
        .await
        .expect("create owner");
    tenancy::add_member(&ctx.db, tenant.id, owner.id, MembershipRole::Owner)
        .await
        .expect("add owner");
    let key = identity_api_keys::Entity::create_api_key(
        &ctx.db,
        workspace.id,
        ApiKeyScope::Schema,
        Some(owner.id),
        false,
    )
    .await
    .expect("issue key")
    .plaintext;
    Setup {
        key,
        tenant_id: tenant.id,
        workspace_id: workspace.id,
    }
}

/// Setting, reading and clearing a workspace's LLM credentials over REST.
/// The key itself never comes back from GET, only what it configured.
#[tokio::test]
#[serial]
async fn llm_key_set_get_and_clear_round_trip() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let missing = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(missing.status_code(), 404, "response: {:?}", missing.text());

        let put = request
            .put("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "base_url": "https://api.example.com/v1/",
                "model": "gpt-4o-mini",
                "api_key": "sk-secret-value"
            }))
            .await;
        assert_eq!(put.status_code(), 204, "response: {:?}", put.text());

        let get = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(get.status_code(), 200, "response: {:?}", get.text());
        let body: serde_json::Value = get.json();
        // The trailing slash is trimmed once at write time.
        assert_eq!(body["base_url"], "https://api.example.com/v1");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["configured"], true);
        let rendered = get.text();
        assert!(
            !rendered.contains("sk-secret-value"),
            "the key must never be returned: {rendered}"
        );

        let delete = request
            .delete("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(delete.status_code(), 204, "response: {:?}", delete.text());

        let after_delete = request
            .get("/hosted/workspace/llm-key")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(after_delete.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A scheme that could never be a chat-completions endpoint is refused before anything is stored, and a URL with no scheme at all is refused too rather than becoming a relative path.
#[tokio::test]
#[serial]
async fn a_non_http_base_url_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        for bad_url in [
            "file:///etc/passwd",
            "gopher://example.com",
            "api.example.com/v1",
        ] {
            let put = request
                .put("/hosted/workspace/llm-key")
                .add_header("Authorization", format!("Bearer {}", setup.key))
                .json(&serde_json::json!({
                    "base_url": bad_url,
                    "model": "m",
                    "api_key": "k"
                }))
                .await;
            assert_eq!(
                put.status_code(),
                422,
                "{bad_url:?} should have been refused: {:?}",
                put.text()
            );
        }

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// A workspace with no credentials configured is refused with one clear error before any entity is scanned, rather than reporting zero proposals in a way that reads as "nothing to infer".
#[tokio::test]
#[serial]
async fn infer_fill_without_a_configured_key_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        licence(&ctx);
        let setup = setup(&ctx).await;

        let create_schema = request
            .post("/api/schemas")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .json(&serde_json::json!({
                "name": "note",
                "entity_types": {
                    "note": { "fields": { "title": { "type": "string", "required": true } } }
                }
            }))
            .await;
        assert_eq!(
            create_schema.status_code(),
            201,
            "response: {:?}",
            create_schema.text()
        );

        let infer = request
            .post("/hosted/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(infer.status_code(), 422, "response: {:?}", infer.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// An unlicensed deployment answers the same 404 whether or not a valid key is presented, so an anonymous prober cannot tell "does not exist" from "exists but locked".
/// Matches `marketplace`'s and `dashboard`'s own tests for the same gate.
#[tokio::test]
#[serial]
async fn an_unlicensed_deployment_answers_the_same_without_a_valid_key() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let with_key = request
            .post("/hosted/schemas/active/note/infer-fill")
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        let without_key = request.post("/hosted/schemas/active/note/infer-fill").await;

        assert_eq!(with_key.status_code(), without_key.status_code());
        assert_eq!(with_key.status_code(), 404);

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Confirming a job with no proposals is refused rather than reporting nothing applied, so a caller can tell "already confirmed" from "confirmed and changed nothing".
#[tokio::test]
#[serial]
async fn confirming_an_unknown_job_is_refused() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let setup = setup(&ctx).await;

        let confirm = request
            .post(&format!(
                "/hosted/migration-jobs/{}/confirm",
                Uuid::new_v4()
            ))
            .add_header("Authorization", format!("Bearer {}", setup.key))
            .await;
        assert_eq!(confirm.status_code(), 404, "response: {:?}", confirm.text());

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// Sets up one schema, one entity and one recorded proposal for the infrastructure-failure tests
/// below, then locks `content_entities` in `lock_mode` from a second connection and calls
/// `confirm` with a short `lock_timeout`, returning `confirm`'s result as a string (its `Err`
/// message, or `"Ok"` if it somehow succeeded) for the caller to assert on.
///
/// yorishiro_app is granted per-table (see loco-architecture.md), not the owner of any table, so
/// it cannot DROP or REVOKE its own way into a failure; locking the table from a second connection
/// is a failure `confirm`'s loop can genuinely hit without needing privileges the RLS role does
/// not have.
async fn confirm_with_content_entities_locked(
    request: &axum_test::TestServer,
    ctx: &loco_rs::app::AppContext,
    lock_mode: &'static str,
) -> String {
    licence(ctx);
    let setup = setup(ctx).await;

    let create_schema = request
        .post("/api/schemas")
        .add_header("Authorization", format!("Bearer {}", setup.key))
        .json(&serde_json::json!({
            "name": "note",
            "entity_types": {
                "note": { "fields": { "title": { "type": "string", "required": true } } }
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
        .add_header("Authorization", format!("Bearer {}", setup.key))
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

    let db = ctx.shared_store.get::<DbHandle>().unwrap();
    let job_id = Uuid::new_v4();
    {
        let txn = db
            .tenant
            .begin_for_workspace(setup.tenant_id, setup.workspace_id)
            .await
            .expect("begin tenant txn");
        fill_proposals::record(
            &txn,
            setup.workspace_id,
            job_id,
            entity_id,
            "title",
            &serde_json::json!("guessed"),
        )
        .await
        .expect("record proposal");
        txn.commit().await.expect("commit proposal");
    }

    let mut blocker = db.identity.acquire().await.expect("acquire blocker conn");
    sqlx::query("BEGIN")
        .execute(&mut *blocker)
        .await
        .expect("begin blocker txn");
    let lock_statement = match lock_mode {
        "EXCLUSIVE" => "LOCK TABLE content_entities IN EXCLUSIVE MODE",
        "ACCESS EXCLUSIVE" => "LOCK TABLE content_entities IN ACCESS EXCLUSIVE MODE",
        other => panic!("unexpected lock mode {other:?}"),
    };
    sqlx::query(lock_statement)
        .execute(&mut *blocker)
        .await
        .expect("lock content_entities");

    let result = {
        let txn = db
            .tenant
            .begin_for_workspace(setup.tenant_id, setup.workspace_id)
            .await
            .expect("begin tenant txn");
        txn.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "SET LOCAL lock_timeout = '50ms'",
        ))
        .await
        .expect("set lock_timeout");

        let result = fill_proposals::confirm(&txn, setup.workspace_id, job_id).await;
        // Rolls back on drop: the lock_timeout is transaction-local, and anything confirm did
        // before failing is undone along with it.
        match result {
            Ok(_) => "Ok".to_string(),
            Err(err) => err.to_string(),
        }
    };

    sqlx::query("ROLLBACK")
        .execute(&mut *blocker)
        .await
        .expect("release blocker lock");
    // The pool does not consider this connection returned until it drops: the next
    // begin_for_workspace call hangs until this one is, so it must go before that call.
    drop(blocker);

    // Proposals survive either way, old code and new: a real Postgres error aborts the whole
    // transaction, so the trailing DELETE FROM content_fill_proposals never runs regardless. This
    // is not itself evidence of a fix; it is checked here only so the fixture is confirmed sane.
    let verify_txn = db
        .tenant
        .begin_for_workspace(setup.tenant_id, setup.workspace_id)
        .await
        .expect("begin tenant txn");
    let remaining = fill_proposals::for_job(&verify_txn, setup.workspace_id, job_id)
        .await
        .expect("list proposals");
    assert_eq!(
        remaining.len(),
        1,
        "the fixture's one proposal should still be there"
    );
    verify_txn.rollback().await.expect("rollback verify txn");

    result
}

/// The enclosing transaction already guarantees proposals survive any genuine infrastructure
/// failure, old code and new alike: a real Postgres error aborts the transaction, so the trailing
/// `DELETE FROM content_fill_proposals` can never run either way. What the old code actually cost
/// was the diagnosis, not the data: `Err(_) => skipped += ...` swallowed the lock timeout below
/// into a plain `skipped` count, the loop moved on, and the next statement (that same `DELETE`)
/// hit "current transaction is aborted" instead of ever running — so the caller received that
/// masked, unrelated-looking error rather than the lock timeout that actually happened. The fix
/// (`Err(err) => return Err(err)`) returns the real error immediately instead.
///
/// `content_entities` is locked `EXCLUSIVE`, not `ACCESS EXCLUSIVE`: `EXCLUSIVE` still admits the
/// plain `SELECT` `content_entities::get` runs earlier in the same loop iteration, so the failure
/// lands specifically on `update`'s write, exercising the `Err(err) => return Err(err)` arm this
/// fix added there rather than the one on `get`. `an_infrastructure_failure_on_get_surfaces_as_itself`
/// below is `get`'s counterpart, with `ACCESS EXCLUSIVE` instead so the failure lands there first.
///
/// Asserting on `"lock timeout"` in the error text matches Postgres's own (English) server
/// message, which depends on the server's `lc_messages`; this suite's container runs
/// `en_US.utf8`, so this is a real but currently-inert brittleness rather than one to engineer
/// around here.
#[tokio::test]
#[serial]
async fn an_infrastructure_failure_surfaces_as_itself_not_a_masked_abort_error() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let message = confirm_with_content_entities_locked(&request, &ctx, "EXCLUSIVE").await;
        // The observable difference between the old code and this fix: the old code let the lock
        // timeout fall into `skipped`, then hit "current transaction is aborted" on the next
        // statement and returned *that* instead. This fix returns the lock timeout itself,
        // immediately, from the `update` call where it actually happened.
        assert!(
            message.contains("lock timeout"),
            "expected the lock timeout itself, not a downstream error: {message}"
        );
        assert!(
            !message.contains("transaction is aborted"),
            "the old code's masked failure mode must not reappear: {message}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}

/// `get`'s counterpart to the `update` test above: `content_entities` is locked
/// `ACCESS EXCLUSIVE`, the one lock mode that also blocks the plain `SELECT`
/// `content_entities::get` runs, so the failure lands on `get` (the very first statement in the
/// loop) rather than on `update`. This isolates the `Err(err) => return Err(err)` arm the fix
/// added to `get`'s own match, which `an_infrastructure_failure_surfaces_as_itself_not_a_masked_abort_error`
/// does not exercise: that test's `EXCLUSIVE` lock still admits `get`'s read, so it only ever
/// proved the `update` arm.
#[tokio::test]
#[serial]
async fn an_infrastructure_failure_on_get_surfaces_as_itself() {
    request_with_create_db::<HostedApp, _, _>(|request, ctx| async move {
        let message =
            confirm_with_content_entities_locked(&request, &ctx, "ACCESS EXCLUSIVE").await;
        assert!(
            message.contains("lock timeout"),
            "expected the lock timeout itself, not a downstream error: {message}"
        );
        assert!(
            !message.contains("transaction is aborted"),
            "the old code's masked failure mode must not reappear: {message}"
        );

        super::close_app_pools(&ctx).await;
    })
    .await;
}
