use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
use sea_orm::{EntityTrait, QueryOrder};
use serde_json::Value;
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::access_keys::WORKSPACE_HEADER;

use crate::requests::fixtures::{self, TenantArgs, TestTenant};
use crate::requests::{boot_request, boot_request_with_timezone};

mod issuance_race;

#[tokio::test]
async fn root_credential_management_requires_a_root_session() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let delegated = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        let (target, _) = yorishiro::models::tenancy::issue_login_key(
            &ctx.db,
            tenant.owner_id,
            None,
            Some(tenant.tenant_id),
        )
        .await
        .unwrap();
        let root = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let path = format!("/api/access-keys/{}", target.id);

        request
            .patch(&path)
            .add_header("Authorization", format!("Bearer {delegated}"))
            .json(&serde_json::json!({"enabled":false}))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .delete(&path)
            .add_header("Authorization", format!("Bearer {delegated}"))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .delete(&path)
            .add_header("Authorization", format!("Bearer {root}"))
            .await
            .assert_status(StatusCode::NO_CONTENT);
    })
    .await;
}

#[tokio::test]
async fn delegated_expiry_cannot_outlive_the_issuing_key() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let tenant_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        let owner = format!("Bearer {tenant_key}");
        let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
        let issued = request.post("/api/access-keys").add_header("Authorization",&owner)
            .json(&serde_json::json!({"name":"short-admin","expires_at":expiry})).await;
        issued.assert_status(StatusCode::CREATED);
        let issued = issued.json::<Value>();
        let auth = format!("Bearer {}",issued["access_key"].as_str().unwrap());
        let inherited = request.post("/api/access-keys").add_header("Authorization",&auth)
            .json(&serde_json::json!({"name":"inherit"})).await;
        inherited.assert_status(StatusCode::CREATED);
        assert_eq!(inherited.json::<Value>()["expires_at"], issued["expires_at"]);
        request.post("/api/access-keys").add_header("Authorization",&auth)
            .json(&serde_json::json!({"name":"longer","expires_at":expiry+chrono::Duration::seconds(1)}))
            .await.assert_status(StatusCode::FORBIDDEN);
        request.post("/api/access-keys").add_header("Authorization",&auth).add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({"name":"bounded","expires_at":expiry-chrono::Duration::seconds(1)}))
            .await.assert_status(StatusCode::CREATED);
        let key = request.post("/api/access-keys").add_header("Authorization",&owner).add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({"name":"unlimited-target"})).await.json::<Value>();
        request.patch(&format!("/api/access-keys/{}",key["access_key_id"].as_str().unwrap()))
            .add_header("Authorization",&auth).json(&serde_json::json!({"replace_expires_at":true})).await.assert_status(StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one credential history compares authorized and denied updates and deletion while tracking one-time plaintext disclosure"
)]
async fn access_key_lifecycle_is_admin_only_and_secret_is_one_time() {
    boot_request_with_timezone::<App, _, _>("Asia/Tokyo", |request, ctx| async move {
        let timezone = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap()
            .timezone;
        assert_ne!(timezone, "UTC", "test must boot with a non-default zone");
        let timezone = timezone.parse::<chrono_tz::Tz>().unwrap();
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let owner_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;

        assert_eq!(
            request.get("/api/access-keys").await.status_code(),
            StatusCode::UNAUTHORIZED
        );

        let created = request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({ "name": "  my-client  " }))
            .await;
        assert_eq!(created.status_code(), StatusCode::CREATED);
        let created_body: Value = created.json();
        assert_eq!(created_body["name"], "my-client");
        assert_eq!(created_body["identity_kind"], "user");
        assert!(created_body["access_key"].as_str().is_some());
        assert!(created_body["prefix"].as_str().unwrap().starts_with("ysr_"));
        assert!(created_body["access_key_id"].as_str().is_some());
        assert!(created_body["created_at"].as_str().is_some());
        let created_at =
            chrono::DateTime::parse_from_rfc3339(created_body["created_at"].as_str().unwrap())
                .unwrap();
        assert_eq!(
            created_at.offset().local_minus_utc(),
            timezone
                .offset_from_utc_datetime(&created_at.naive_utc())
                .fix()
                .local_minus_utc()
        );

        let full_key = created_body["access_key"].as_str().unwrap().to_owned();
        let identity = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(identity.status_code(), StatusCode::OK);
        let identity = identity.json::<Value>();
        assert_eq!(identity["identity_kind"], created_body["identity_kind"]);
        let unsupported = request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({"name":"unsupported", "binding":"tenant"}))
            .await;
        assert_eq!(unsupported.status_code(), StatusCode::UNPROCESSABLE_ENTITY);
        let parts: Vec<_> = full_key.split('_').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "ysr");
        assert_eq!(parts[1].len(), 12);
        assert_eq!(parts[2].len(), 48);
        assert!(
            parts[1..]
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        );
        let listed = request
            .get("/api/access-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed_body: Value = listed.json();
        assert!(listed_body["keys"].as_array().unwrap().iter().any(|key| {
            key["prefix"]
                .as_str()
                .is_some_and(|prefix| owner_key.starts_with(prefix))
        }));
        assert!(listed_body.to_string().contains("my-client"));
        assert!(!listed_body.to_string().contains(&full_key));
        assert!(!listed_body.to_string().contains("key_hash"));
        let record = listed_body["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["access_key_id"] == created_body["access_key_id"])
            .unwrap();
        assert_eq!(record["identity_kind"], created_body["identity_kind"]);
        assert_eq!(record["workspace_id"], created_body["workspace_id"]);
        assert_eq!(record["tenant_id"], created_body["tenant_id"]);
        assert_eq!(record["user_id"], created_body["user_id"]);
        for created_at in listed_body["keys"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|key| key["created_at"].as_str())
        {
            let created_at = chrono::DateTime::parse_from_rfc3339(created_at).unwrap();
            assert_eq!(
                created_at.offset().local_minus_utc(),
                timezone
                    .offset_from_utc_datetime(&created_at.naive_utc())
                    .fix()
                    .local_minus_utc()
            );
        }

        let authenticated = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(authenticated.status_code(), StatusCode::OK);

        let access_key_id: Uuid = created_body["access_key_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let revoked = request
            .delete(&format!("/api/access-keys/{access_key_id}"))
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(revoked.status_code(), StatusCode::NO_CONTENT);

        let no_longer_authenticated = request
            .get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(
            no_longer_authenticated.status_code(),
            StatusCode::UNAUTHORIZED
        );

        assert_eq!(
            request
                .delete(&format!("/api/access-keys/{access_key_id}"))
                .add_header("Authorization", format!("Bearer {owner_key}"))
                .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
                .await
                .status_code(),
            StatusCode::NOT_FOUND
        );

        let invalid = request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({ "name": "   " }))
            .await;
        assert_eq!(invalid.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

        let missing = request
            .delete("/api/access-keys/not-a-uuid")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await;
        assert_eq!(missing.status_code(), StatusCode::BAD_REQUEST);
    })
    .await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the same tenant compares issuer roles and credential bindings before using and listing the issued keys"
)]
async fn tenant_scoped_issuance_keeps_binding_and_admin_ceiling() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::{access_keys, tenancy};

    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let issuer = access_keys::Entity::create_tenant_access_key(
            &ctx.db,
            tenant.tenant_id,
            Some(administrator),
            "initial-admin",
        )
        .await
        .expect("bootstrap tenant-scoped key");
        let created = request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .json(&serde_json::json!({
                "name": "tenant-scoped",
            }))
            .await;
        created.assert_status(StatusCode::CREATED);
        let record = created.json::<Value>();
        assert!(record["workspace_id"].is_null(), "response: {record:?}");
        assert_eq!(record["tenant_id"], tenant.tenant_id.to_string());

        // A non-admin member cannot issue keys.
        let txn = ctx.db.begin().await.unwrap();
        let member = tenancy::register_member(
            &txn,
            tenant.tenant_id,
            &fixtures::unique_email("tenant-scoped-member"),
            "hunter2-hunter2",
            None)
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let member_key = fixtures::issue_access_key(
            &ctx,
            tenant.workspace_id,
            member.id,
        )
        .await;
        request
            .post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({"name": "member-tenant"}))
            .await
            .assert_status(StatusCode::FORBIDDEN);

        let member_tenant_key = access_keys::Entity::create_tenant_access_key(
            &ctx.db, tenant.tenant_id,
            Some(member.id), "member-tenant-issuer",
        ).await.unwrap();
        let _elevated = access_keys::Entity::create_tenant_access_key(
            &ctx.db, tenant.tenant_id,
            Some(member.id), "iam-not-role-ceiling",
        ).await.unwrap();
        let document = serde_json::json!({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:CreateAccessKey",
            "Resource":format!("yrn:tenant:{}", tenant.tenant_id)}]});
        let policy = request.post("/api/policies")
            .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .json(&serde_json::json!({"name":"member-issuance","document":document})).await;
        policy.assert_status(StatusCode::CREATED);
        let policy_id = policy.json::<Value>()["policy_id"].as_str().unwrap().to_owned();
        let attachment = format!("/api/users/{}/policies/{policy_id}", member.id);
        request.put(&attachment).add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .await.assert_status(StatusCode::NO_CONTENT);
        request.post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", member_tenant_key.plaintext))
            .json(&serde_json::json!({"name":"iam-member-write"}))
            .await.assert_status(StatusCode::CREATED);
        request.delete(&attachment).add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .await.assert_status(StatusCode::NO_CONTENT);
        request.post("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", member_tenant_key.plaintext))
            .json(&serde_json::json!({"name":"denied-member"}))
            .await.assert_status(StatusCode::FORBIDDEN);

        let txn = ctx.db.begin().await.unwrap();
        let inaccessible = tenancy::create_workspace(&txn, tenant.tenant_id, "no-policy", None).await.unwrap();
        txn.commit().await.unwrap();
        let ungranted = fixtures::create_key(
            &ctx.db, inaccessible.id, Some(member.id),
        ).await.expect("workspace-bound issuance succeeds");
        // The key authenticates, but without a policy for the workspace it reads nothing.
        request.get("/api/entities")
            .add_header("Authorization", format!("Bearer {}", ungranted.plaintext))
            .add_header(WORKSPACE_HEADER, inaccessible.id.to_string())
            .await.assert_status(StatusCode::NOT_FOUND);
        request.get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {}", member_tenant_key.plaintext))
            .add_header(WORKSPACE_HEADER, inaccessible.id.to_string())
            .await.assert_status(StatusCode::OK);
        let denied = request.get(&format!("/api/workspaces/{}", inaccessible.id))
            .add_header("Authorization", format!("Bearer {}", member_tenant_key.plaintext))
            .await;
        denied.assert_status(StatusCode::NOT_FOUND);
        let missing = request.get(&format!("/api/workspaces/{}", Uuid::now_v7()))
            .add_header("Authorization", format!("Bearer {}", member_tenant_key.plaintext))
            .await;
        missing.assert_status(StatusCode::NOT_FOUND);
        assert_eq!(denied.json::<Value>(), missing.json::<Value>());
        let listed = request.get("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await.json::<Value>();
        assert!(listed["keys"].as_array().unwrap().iter()
            .any(|key| key["access_key_id"] == member_tenant_key.id.to_string()));

        // The new key names its workspace per request and appears in the listing.
        let full_key = record["access_key"].as_str().unwrap().to_owned();
        let txn = ctx.db.begin().await.unwrap();
        let second_name = format!("second-key-target-{}", Uuid::now_v7());
        let second = tenancy::create_workspace(&txn, tenant.tenant_id, &second_name, None).await.unwrap();
        txn.commit().await.unwrap();
        request.get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .add_header(WORKSPACE_HEADER, second.id.to_string())
            .await.assert_status(StatusCode::OK);
        request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await
            .assert_status(StatusCode::OK);
        let listed = request
            .get("/api/access-keys")
            .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await;
        listed.assert_status(StatusCode::OK);
        let keys = listed.json::<Value>();
        assert!(
            keys["keys"]
                .as_array()
                .unwrap()
                .iter()
                .any(|key| key["access_key_id"] == record["access_key_id"])
        );
        request
            .delete(&format!("/api/access-keys/{}", record["access_key_id"].as_str().unwrap()))
            .add_header("Authorization", format!("Bearer {}", issuer.plaintext))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await
            .assert_status(StatusCode::NO_CONTENT);
        request.get("/api/caller-identity")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .await.assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn update_changes_name_expiry_and_enabled_flag() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let tenant_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, administrator).await;
        let auth = format!("Bearer {tenant_key}");
        let created = request
            .post("/api/access-keys")
            .add_header("Authorization", &auth)
            .add_header(WORKSPACE_HEADER, tenant.workspace_id.to_string())
            .json(&serde_json::json!({"name":"before"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let created: Value = created.json();
        let path = format!("/api/access-keys/{}", created["access_key_id"].as_str().unwrap());
        let secret = format!("Bearer {}", created["access_key"].as_str().unwrap());

        let expiry = chrono::Utc::now() + chrono::Duration::hours(2);
        let updated = request
            .patch(&path)
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":" after ","expires_at":expiry,"replace_expires_at":true}))
            .await;
        updated.assert_status(StatusCode::OK);
        let updated: Value = updated.json();
        assert_eq!(updated["name"], "after");
        assert!(updated["expires_at"].is_string());
        assert_eq!(updated["enabled"], true);
        assert_eq!(updated["state"], "active");

        let disabled = request
            .patch(&path)
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"enabled":false}))
            .await;
        disabled.assert_status(StatusCode::OK);
        assert_eq!(disabled.json::<Value>()["state"], "disabled");
        request
            .get("/api/caller-identity")
            .add_header("Authorization", &secret)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let enabled = request
            .patch(&path)
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"enabled":true,"replace_expires_at":true}))
            .await;
        enabled.assert_status(StatusCode::OK);
        let enabled: Value = enabled.json();
        assert!(enabled["expires_at"].is_null());
        request
            .get("/api/caller-identity")
            .add_header("Authorization", &secret)
            .await
            .assert_status(StatusCode::OK);

        for body in [
            serde_json::json!({"name":"  "}),
            serde_json::json!({"expires_at":chrono::Utc::now()-chrono::Duration::seconds(1),"replace_expires_at":true}),
            serde_json::json!({"unknown":1}),
        ] {
            request
                .patch(&path)
                .add_header("Authorization", &auth)
                .json(&body)
                .await
                .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
        }
        request
            .patch(&format!("/api/access-keys/{}", Uuid::new_v4()))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":"missing"}))
            .await
            .assert_status(StatusCode::NOT_FOUND);
        request.delete(&path).add_header("Authorization", &auth)
            .await.assert_status(StatusCode::NO_CONTENT);
        let actions: Vec<String> = yorishiro::models::api_events::Entity::find()
            .order_by_asc(yorishiro::models::_entities::api_events::Column::CreatedAt)
            .all(&ctx.db)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.phase == "completed")
            .flat_map(|event| event.resources.as_array().cloned().unwrap_or_default())
            .filter(|resource| resource["detail"]["access_key_id"] == created["access_key_id"])
            .filter_map(|resource| resource["action"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(actions, ["iam:CreateAccessKey", "iam:UpdateAccessKey", "iam:UpdateAccessKey", "iam:UpdateAccessKey", "iam:DeleteAccessKey"]);
    })
    .await;
}
