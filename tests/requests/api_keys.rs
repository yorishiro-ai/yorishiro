use axum::http::StatusCode;
use chrono::{Offset, TimeZone};
use serde_json::Value;
use uuid::Uuid;
use yorishiro::App;

use super::fixtures::{self, TenantArgs, TestTenant};
use super::{boot_request, boot_request_with_timezone};

#[tokio::test]
async fn scope_rejection_does_not_touch_a_key_but_a_read_does() {
    use yorishiro::models::api_keys::{ApiKeyScope, Entity};

    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            workspace_id,
            owner_id,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key =
            fixtures::issue_api_key(&ctx, workspace_id, owner_id, ApiKeyScope::Read, false).await;
        let before = Entity::list_for_workspace(&ctx.db, workspace_id, Default::default())
            .await
            .expect("list workspace keys")
            .into_iter()
            .find(|key| key.scope == "read")
            .expect("read key");
        assert!(before.last_used_at.is_none());
        let denied = request
            .post("/api/entities")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"entity_type":"note", "data":{}}))
            .await;
        assert_eq!(denied.status_code(), StatusCode::FORBIDDEN);
        assert!(
            Entity::list_for_workspace(&ctx.db, workspace_id, Default::default())
                .await
                .expect("list workspace keys")
                .into_iter()
                .find(|key| key.id == before.id)
                .expect("read key remains")
                .last_used_at
                .is_none()
        );
        let read = request
            .get("/api/workspaces")
            .add_header("Authorization", format!("Bearer {key}"))
            .await;
        assert_eq!(read.status_code(), StatusCode::OK);
        assert!(
            Entity::list_for_workspace(&ctx.db, workspace_id, Default::default())
                .await
                .expect("list workspace keys")
                .into_iter()
                .find(|key| key.id == before.id)
                .expect("read key remains")
                .last_used_at
                .is_some()
        );
    })
    .await;
}

#[tokio::test]
async fn workspace_bound_issuer_cannot_rotate_another_workspaces_key() {
    use sea_orm::TransactionTrait;
    use yorishiro::models::{api_keys, tenancy};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let txn = ctx.db.begin().await.unwrap();
        let sibling = tenancy::create_workspace(
            &txn,
            tenant.tenant_id,
            &format!("rotation-target-{}", Uuid::new_v4()),
            None,
            None,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        let target = api_keys::Entity::create_api_key(
            &ctx.db,
            sibling.id,
            api_keys::ApiKeyScope::Read,
            Some(tenant.owner_id),
            false,
        )
        .await
        .unwrap();
        request
            .post(&format!("/api/api-keys/{}/rotate", target.id))
            .add_header("Authorization", format!("Bearer {}", tenant.api_key))
            .await
            .assert_status(StatusCode::FORBIDDEN);
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", target.plaintext))
            .await
            .assert_status(StatusCode::OK);
    })
    .await;
}

#[tokio::test]
async fn delegated_expiry_cannot_outlive_the_issuing_key() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let owner = format!("Bearer {}",tenant.api_key);
        let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
        let issued = request.post("/api/api-keys").add_header("Authorization",&owner)
            .json(&serde_json::json!({"name":"short-admin","expires_at":expiry})).await;
        issued.assert_status(StatusCode::CREATED);
        let issued = issued.json::<Value>();
        let auth = format!("Bearer {}",issued["full_key"].as_str().unwrap());
        for body in [serde_json::json!({"name":"unlimited"}),
            serde_json::json!({"name":"longer","expires_at":expiry+chrono::Duration::seconds(1)})] {
            request.post("/api/api-keys").add_header("Authorization",&auth).json(&body)
                .await.assert_status(StatusCode::FORBIDDEN);
        }
        request.post("/api/api-keys").add_header("Authorization",&auth)
            .json(&serde_json::json!({"name":"bounded","expires_at":expiry-chrono::Duration::seconds(1)}))
            .await.assert_status(StatusCode::CREATED);
        let key = request.post("/api/api-keys").add_header("Authorization",&owner)
            .json(&serde_json::json!({"name":"unlimited-target"})).await.json::<Value>();
        request.post(&format!("/api/api-keys/{}/rotate",key["id"].as_str().unwrap()))
            .add_header("Authorization",&auth).await.assert_status(StatusCode::FORBIDDEN);
    }).await;
}

#[tokio::test]
async fn rotation_preserves_service_identity_and_serializes_with_revocation() {
    use yorishiro::models::api_keys::{ApiKeyScope, Entity};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let service =
            Entity::create_api_key(&ctx.db, tenant.workspace_id, ApiKeyScope::Read, None, false)
                .await
                .unwrap();
        let auth = format!("Bearer {}", tenant.api_key);
        let path = format!("/api/api-keys/{}/rotate", service.id);
        let (rotated, revoked) = tokio::join!(
            async { request.post(&path).add_header("Authorization", &auth).await },
            Entity::revoke(&ctx.db, service.id)
        );
        if rotated.status_code() == StatusCode::CREATED {
            let body = rotated.json::<Value>();
            assert_eq!(body["identity_kind"], "service");
            assert!(body["user_id"].is_null());
            assert_eq!(body["scope"], "read");
            assert!(revoked.is_err());
        } else {
            rotated.assert_status(StatusCode::CONFLICT);
            revoked.unwrap();
        }
        request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {}", service.plaintext))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    })
    .await;
}

#[tokio::test]
async fn concurrent_rotation_issues_only_one_replacement() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let auth = format!("Bearer {}", tenant.api_key);
        let created = request
            .post("/api/api-keys")
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":"race"}))
            .await;
        created.assert_status(StatusCode::CREATED);
        let id = created.json::<Value>()["id"].as_str().unwrap().to_owned();
        let path = format!("/api/api-keys/{id}/rotate");
        let (a, b) = tokio::join!(
            async { request.post(&path).add_header("Authorization", &auth).await },
            async { request.post(&path).add_header("Authorization", &auth).await }
        );
        let mut statuses = [a.status_code().as_u16(), b.status_code().as_u16()];
        statuses.sort();
        assert_eq!(statuses, [201, 409]);
        let listed = request
            .get("/api/api-keys")
            .add_header("Authorization", &auth)
            .await
            .json::<Value>();
        assert_eq!(
            listed["keys"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|key| key["name"] == "race" && key["state"] == "active")
                .count(),
            1
        );
    })
    .await;
}

#[tokio::test]
async fn key_rotation_preserves_grants_and_retains_secret_free_history() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let auth = format!("Bearer {}",tenant.api_key);
        let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
        let created = request.post("/api/api-keys").add_header("Authorization",&auth)
            .json(&serde_json::json!({"name":"rotate-me","scope":"read","expires_at":expiry})).await;
        created.assert_status(StatusCode::CREATED);
        let original: Value = created.json();
        let id = original["id"].as_str().unwrap();
        let rotated = request.post(&format!("/api/api-keys/{id}/rotate"))
            .add_header("Authorization",&auth).await;
        rotated.assert_status(StatusCode::CREATED);
        let replacement: Value = rotated.json();
        for field in ["scope","audit","workspace_id","tenant_id","user_id","expires_at"] {
            assert_eq!(original[field],replacement[field]);
        }
        request.get("/api/whoami").add_header("Authorization",format!("Bearer {}",original["full_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::UNAUTHORIZED);
        request.get("/api/whoami").add_header("Authorization",format!("Bearer {}",replacement["full_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::OK);
        request.post(&format!("/api/api-keys/{id}/rotate")).add_header("Authorization",&auth)
            .await.assert_status(StatusCode::CONFLICT);
        let listed = request.get("/api/api-keys").add_header("Authorization",&auth).await.json::<Value>();
        let old = listed["keys"].as_array().unwrap().iter().find(|key|key["id"]==original["id"]).unwrap();
        assert_eq!(old["state"],"revoked");
        assert_eq!(old["replaced_by_id"],replacement["id"]);
        assert!(old["effective_scope"].is_null());
        assert!(!listed.to_string().contains(original["full_key"].as_str().unwrap()));
        assert!(!listed.to_string().contains("key_hash"));
        use sea_orm::{ActiveModelTrait, ActiveValue::Set};
        yorishiro::models::api_keys::ActiveModel {
            id: Set(replacement["id"].as_str().unwrap().parse().unwrap()),
            expires_at: Set(Some((chrono::Utc::now()-chrono::Duration::seconds(1)).fixed_offset())),
            ..Default::default()
        }.update(&ctx.db).await.unwrap();
        request.get("/api/whoami").add_header("Authorization",format!("Bearer {}",replacement["full_key"].as_str().unwrap()))
            .await.assert_status(StatusCode::UNAUTHORIZED);
        let expired = request.get("/api/api-keys").add_header("Authorization",&auth).await.json::<Value>();
        let expired = expired["keys"].as_array().unwrap().iter().find(|key|key["id"]==replacement["id"]).unwrap();
        assert_eq!(expired["state"],"expired");
        assert!(expired["effective_scope"].is_null());
        request.post("/api/api-keys").add_header("Authorization",&auth)
            .json(&serde_json::json!({"name":"past","expires_at":chrono::Utc::now()-chrono::Duration::seconds(1)}))
            .await.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    }).await;
}

#[tokio::test]
async fn api_key_lifecycle_is_admin_only_and_secret_is_one_time() {
    boot_request_with_timezone::<App, _, _>("Asia/Tokyo", |request, ctx| async move {
        let timezone = ctx
            .config
            .settings::<yorishiro::data::settings::Settings>()
            .unwrap()
            .timezone;
        assert_ne!(timezone, "UTC", "test must boot with a non-default zone");
        let timezone = timezone.parse::<chrono_tz::Tz>().unwrap();
        let TestTenant {
            api_key: owner_key, ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;

        assert_eq!(
            request.get("/api/api-keys").await.status_code(),
            StatusCode::UNAUTHORIZED
        );

        let created = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "name": "  my-client  " }))
            .await;
        assert_eq!(created.status_code(), StatusCode::CREATED);
        let created_body: Value = created.json();
        assert_eq!(created_body["name"], "my-client");
        assert_eq!(created_body["scope"], "read");
        assert_eq!(created_body["effective_scope"], "read");
        assert_eq!(created_body["identity_kind"], "user");
        assert_eq!(created_body["audit"], false);
        assert!(created_body["full_key"].as_str().is_some());
        assert!(created_body["prefix"].as_str().unwrap().starts_with("ysr_"));
        assert!(created_body["id"].as_str().is_some());
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

        let full_key = created_body["full_key"].as_str().unwrap().to_owned();
        let identity = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(identity.status_code(), StatusCode::OK);
        assert_eq!(identity.json::<Value>()["scope"], created_body["scope"]);
        let identity = identity.json::<Value>();
        assert_eq!(identity["scope"], created_body["effective_scope"]);
        assert_eq!(identity["stored_scope"], created_body["scope"]);
        assert_eq!(identity["audit"], created_body["audit"]);
        assert_eq!(identity["bound_workspace_id"], created_body["workspace_id"]);
        assert_eq!(identity["identity_kind"], created_body["identity_kind"]);
        let escalation = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .json(&serde_json::json!({"name":"escalation", "scope":"migration"}))
            .await;
        assert_eq!(escalation.status_code(), StatusCode::FORBIDDEN);
        let unsupported = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
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
            .get("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(listed.status_code(), StatusCode::OK);
        let listed_body: Value = listed.json();
        assert_eq!(listed_body["keys"].as_array().unwrap().len(), 2);
        assert!(listed_body.to_string().contains("my-client"));
        assert!(!listed_body.to_string().contains(&full_key));
        assert!(!listed_body.to_string().contains("key_hash"));
        let record = listed_body["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["id"] == created_body["id"])
            .unwrap();
        assert_eq!(record["scope"], "read");
        assert_eq!(record["effective_scope"], created_body["effective_scope"]);
        assert_eq!(record["identity_kind"], created_body["identity_kind"]);
        assert_eq!(record["audit"], created_body["audit"]);
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
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(authenticated.status_code(), StatusCode::OK);

        let key_id: Uuid = created_body["id"].as_str().unwrap().parse().unwrap();
        let revoked = request
            .delete(&format!("/api/api-keys/{key_id}"))
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(revoked.status_code(), StatusCode::NO_CONTENT);

        let no_longer_authenticated = request
            .get("/api/whoami")
            .add_header("Authorization", format!("Bearer {full_key}"))
            .await;
        assert_eq!(
            no_longer_authenticated.status_code(),
            StatusCode::UNAUTHORIZED
        );

        assert_eq!(
            request
                .delete(&format!("/api/api-keys/{key_id}"))
                .add_header("Authorization", format!("Bearer {owner_key}"))
                .await
                .status_code(),
            StatusCode::NOT_FOUND
        );

        let invalid = request
            .post("/api/api-keys")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .json(&serde_json::json!({ "name": "   " }))
            .await;
        assert_eq!(invalid.status_code(), StatusCode::UNPROCESSABLE_ENTITY);

        let missing = request
            .delete("/api/api-keys/not-a-uuid")
            .add_header("Authorization", format!("Bearer {owner_key}"))
            .await;
        assert_eq!(missing.status_code(), StatusCode::BAD_REQUEST);
    })
    .await;
}

#[tokio::test]
async fn api_key_lifecycle_rejects_non_admin_and_cross_tenant_ids() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let TestTenant {
            tenant_id,
            workspace_id,
            api_key: _owner_key,
            ..
        } = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &crate::requests::fixtures::unique_email("member"),
            "member-password",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(
            &ctx.db,
            tenant_id,
            member.id,
            yorishiro::models::tenant_memberships::MembershipRole::Member,
        )
        .await
        .expect("add member");
        super::fixtures::grant_workspace(
            &ctx,
            workspace_id,
            member.id,
            yorishiro::models::workspace_memberships::WorkspaceRole::Member,
        )
        .await;
        let member_key = yorishiro::models::api_keys::Entity::create_api_key(
            &ctx.db,
            workspace_id,
            yorishiro::models::api_keys::ApiKeyScope::Read,
            Some(member.id),
            false,
        )
        .await
        .expect("issue member key")
        .plaintext;
        let forbidden = request
            .get("/api/api-keys")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .await;
        assert_eq!(forbidden.status_code(), StatusCode::FORBIDDEN);

        // Cross-tenant IDs require a licensed PostgreSQL deployment.
        #[cfg(feature = "enterprise")]
        {
            if !crate::require_postgres_backend() {
                return;
            }
            let TestTenant {
                workspace_id: other_workspace,
                api_key: other_key,
                ..
            } = TestTenant::create(
                &request,
                &ctx,
                TenantArgs {
                    tenant_name: "other".into(),
                    workspace_name: "other".into(),
                    owner_email: super::fixtures::unique_email("other"),
                    ..TenantArgs::default()
                },
            )
            .await;

            let other_created = request
                .post("/api/api-keys")
                .add_header("Authorization", format!("Bearer {other_key}"))
                .json(&serde_json::json!({ "name": "other-client" }))
                .await;
            assert_eq!(other_created.status_code(), StatusCode::CREATED);
            let other_id = other_created.json::<Value>()["id"]
                .as_str()
                .unwrap()
                .to_owned();

            let cross_tenant = request
                .delete(&format!("/api/api-keys/{other_id}"))
                .add_header("Authorization", format!("Bearer {_owner_key}"))
                .await;
            assert_eq!(cross_tenant.status_code(), StatusCode::NOT_FOUND);
            request
                .post(&format!("/api/api-keys/{other_id}/rotate"))
                .add_header("Authorization", format!("Bearer {_owner_key}"))
                .await
                .assert_status(StatusCode::NOT_FOUND);

            let malformed_workspace = request
                .delete(&format!("/api/api-keys/{other_workspace}"))
                .add_header("Authorization", format!("Bearer {_owner_key}"))
                .await;
            assert_eq!(malformed_workspace.status_code(), StatusCode::NOT_FOUND);
        }
    })
    .await;
}
