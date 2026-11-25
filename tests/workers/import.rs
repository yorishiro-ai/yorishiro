use sea_orm::TransactionTrait;
use uuid::Uuid;

use crate::requests::boot_request;
use crate::requests::jobs::finish_job;

#[tokio::test]
async fn queued_import_rechecks_revoked_credentials_before_writing() {
    queued_import_rechecks_inactive_credentials(false).await;
}

#[tokio::test]
async fn queued_import_rechecks_expired_credentials_before_writing() {
    queued_import_rechecks_inactive_credentials(true).await;
}

#[expect(
    clippy::future_not_send,
    reason = "test harness supports thread-local HTTP futures, environment guards, and callbacks"
)]
async fn queued_import_rechecks_inactive_credentials(expired: bool) {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys, schema_schemas};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs {
                key_scope: api_keys::ApiKeyScope::Schema,
                ..Default::default()
            },
        )
        .await;
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Schema,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap();
        let jsonl = json!({"kind":"schema", "record":{
            "id": Uuid::new_v4(), "tenant_id":tenant.tenant_id, "workspace_id":tenant.workspace_id,
            "name":"revoked-import", "version":1,
            "definition":{"name":"revoked-import","entity_types":{},"relation_types":{}},
            "status":"active", "origin_template_id":null, "origin_status":"detached",
            "origin_snapshot":null, "origin_updated_at":null, "created_at":chrono::Utc::now()
        }})
        .to_string();
        let response = request
            .post("/api/import.jsonl")
            .add_header("Authorization", format!("Bearer {}", key.plaintext))
            .text(jsonl)
            .await;
        assert_eq!(
            response.status_code(),
            StatusCode::ACCEPTED,
            "{}",
            response.text()
        );
        let id = response.json::<serde_json::Value>()["job_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        if expired {
            use sea_orm::{ActiveModelTrait, ActiveValue::Set};
            // Direct write (c): the key must expire between the request and the job finishing, which issuance cannot produce.
            api_keys::ActiveModel {
                id: Set(key.id),
                expires_at: Set(Some(
                    (chrono::Utc::now() - chrono::Duration::seconds(1)).fixed_offset(),
                )),
                ..Default::default()
            }
            .update(&ctx.db)
            .await
            .unwrap();
        } else {
            api_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        }
        finish_job(
            &ctx,
            id,
            "failed",
            Some((&request, &tenant.api_key, &format!("/api/import-jobs/{id}"))),
        )
        .await;
        assert!(
            schema_schemas::get_active_schema(&ctx.db, tenant.workspace_id, "revoked-import")
                .await
                .is_err()
        );
    })
    .await;
}

#[tokio::test]
async fn queued_import_rechecks_iam_policies_before_writing() {
    use axum::http::StatusCode;
    use serde_json::json;
    use yorishiro::models::{api_keys, schema_schemas, tenancy};

    boot_request::<yorishiro::App, _, _>(|request, ctx| async move {
        let tenant = crate::requests::fixtures::TestTenant::create(
            &request,
            &ctx,
            crate::requests::fixtures::TenantArgs::default(),
        )
        .await;
        let txn = ctx.db.begin().await.unwrap();
        // A policy-free member: an admin carries the built-in wildcard policy and would survive the attachment removal.
        let member = tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &crate::requests::fixtures::unique_email("queued-workspace"),
            "hunter2-hunter2",
            None)
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let key = api_keys::Entity::create_api_key(
            &ctx.db,
            tenant.workspace_id,
            api_keys::ApiKeyScope::Schema,
            Some(member.id),
            false,
        )
        .await
        .unwrap();
        let jsonl = json!({"kind":"schema", "record":{
            "id":Uuid::new_v4(), "tenant_id":tenant.tenant_id, "workspace_id":tenant.workspace_id,
            "name":"grant-revoked-import", "version":1,
            "definition":{"name":"grant-revoked-import","entity_types":{},"relation_types":{}},
            "status":"active", "origin_template_id":null,"origin_status":"detached",
            "origin_snapshot":null,"origin_updated_at":null,"created_at":chrono::Utc::now()
        }})
        .to_string();
        let iam_key = crate::requests::fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let document = json!({"statements":[{"effect":"allow","actions":["import_create"],
            "resources":[{"type":"workspace","tenant_id":tenant.tenant_id,"workspace_id":tenant.workspace_id}]}]});
        let policy = request.post("/api/iam/policies")
            .add_header("Authorization", format!("Bearer {iam_key}"))
            .json(&json!({"name":"queued-import","document":document})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<serde_json::Value>()["id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/iam/users/{}/policies/{policy_id}", member.id);
        let session = crate::controllers::mcp::initialize(&request).await;
        for explicit_deny in [false, true] {
            request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            let id: Uuid = if explicit_deny {
                let queued = crate::controllers::mcp::call_tool(&request, &session, &key.plaintext,
                    "import_jsonl", json!({"jsonl":jsonl})).await;
                assert_ne!(queued["result"]["isError"], true, "{queued}");
                let queued: serde_json::Value = serde_json::from_str(queued["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                queued["job_id"].as_str().unwrap().parse().unwrap()
            } else {
                let queued = request.post("/api/import.jsonl")
                    .add_header("Authorization", format!("Bearer {}", key.plaintext))
                    .text(jsonl.clone()).await;
                queued.assert_status(StatusCode::ACCEPTED);
                queued.json::<serde_json::Value>()["job_id"].as_str().unwrap().parse().unwrap()
            };
            if explicit_deny {
                let mut deny = document.clone();
                deny["statements"][0]["effect"] = json!("deny");
                request.put(&format!("/api/iam/policies/{policy_id}"))
                    .add_header("Authorization", format!("Bearer {iam_key}"))
                    .json(&json!({"name":"queued-import","document":deny})).await.assert_status_ok();
            }
            request.delete(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                .await.assert_status(StatusCode::NO_CONTENT);
            if explicit_deny {
                request.put(&attachment).add_header("Authorization", format!("Bearer {iam_key}"))
                    .await.assert_status(StatusCode::NO_CONTENT);
            }
            finish_job(&ctx, id, "failed", Some((&request, &tenant.api_key, &format!("/api/import-jobs/{id}")))).await;
            assert!(schema_schemas::get_active_schema(&ctx.db, tenant.workspace_id, "grant-revoked-import").await.is_err());
        }
    }).await;
}
