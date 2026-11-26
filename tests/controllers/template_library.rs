use crate::requests::boot_request;
use axum::http::StatusCode;
use yorishiro::App;

use crate::requests::fixtures::{self, TenantArgs, TestTenant, issue_access_key};

fn note_definition() -> serde_json::Value {
    fixtures::note_schema("library-note", false)
}

#[tokio::test]
async fn template_update_rejects_a_revoked_previously_authenticated_actor() {
    use yorishiro::models::access_keys;
    use yorishiro::models::template_templates::{self, UpdateTemplateInput};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let tenant_key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let created = request.post("/api/template-library")
            .add_header("Authorization", format!("Bearer {tenant_key}"))
            .json(&serde_json::json!({"name":format!("fenced-{}",uuid::Uuid::new_v4()),"definition":note_definition()})).await;
        created.assert_status(StatusCode::CREATED);
        let id = created.json::<serde_json::Value>()["id"].as_str().unwrap().parse().unwrap();
        let administrator = fixtures::create_iam_administrator(&ctx, tenant.tenant_id).await;
        let key = access_keys::Entity::create_tenant_access_key(&ctx.db, tenant.tenant_id, Some(administrator), "template-admin").await.unwrap();
        let actor = access_keys::AuthContext { access_key_id:key.id, workspace_id:None,
            tenant_id:tenant.tenant_id, user_id:Some(administrator), };
        template_templates::update_as(&ctx.db, &actor, id, UpdateTemplateInput {
            name: None, description: Some("before-revocation".into()), definition: None, tags: None, locale: None,
        }).await.unwrap();
        let input = || UpdateTemplateInput { name:None,description:Some("must-not-save".into()),definition:None,tags:None,locale:None };
        access_keys::Entity::revoke(&ctx.db, key.id).await.unwrap();
        assert!(matches!(template_templates::update_as(&ctx.db, &actor, id, input()).await,
            Err(yorishiro::YorishiroError::Unauthenticated)));
        let stored = template_templates::get_template(&ctx.db, tenant.tenant_id, id).await.unwrap();
        assert_eq!(stored.description.as_deref(), Some("before-revocation"));
    }).await;
}

#[tokio::test]
async fn member_role_cannot_manage_the_template_library() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let member = yorishiro::models::user_users::create_user(
            &ctx.db,
            &fixtures::unique_email("member"),
            "hunter2-hunter2",
            None,
        )
        .await
        .expect("create member");
        yorishiro::models::tenant_memberships::add_member(&ctx.db, tenant.tenant_id, member.id)
            .await
            .expect("add member");
        let member_key = issue_access_key(&ctx, tenant.workspace_id, member.id).await;

        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {member_key}"))
            .json(&serde_json::json!({
                "name": "library-note",
                "definition": note_definition(),
            }))
            .await;
        assert_eq!(response.status_code(), StatusCode::FORBIDDEN);
    })
    .await;
}

#[tokio::test]
async fn creating_or_forking_onto_a_taken_template_name_conflicts() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        let auth = format!("Bearer {key}");
        let name = format!("taken-{}", uuid::Uuid::new_v4());
        let create = || async {
            request
                .post("/api/template-library")
                .add_header("Authorization", &auth)
                .json(&serde_json::json!({"name":name,"definition":note_definition()}))
                .await
        };
        let created = create().await;
        created.assert_status(StatusCode::CREATED);
        let id = created.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let message = format!("a template named '{name}' already exists for this tenant");

        let duplicate = create().await;
        duplicate.assert_status(StatusCode::CONFLICT);
        assert!(duplicate.text().contains(&message), "{}", duplicate.text());

        let fork = request
            .post(&format!("/api/template-library/{id}/fork"))
            .add_header("Authorization", &auth)
            .json(&serde_json::json!({"name":name}))
            .await;
        fork.assert_status(StatusCode::CONFLICT);
        assert!(fork.text().contains(&message), "{}", fork.text());
    })
    .await;
}

/// Only a unique violation is a conflict: any other failed insert stays an internal error.
#[tokio::test]
async fn a_non_unique_insert_failure_is_internal_not_a_conflict() {
    use sea_orm::{ConnectionTrait, DbBackend};
    boot_request::<App, _, _>(|request, ctx| async move {
        let tenant = TestTenant::create(&request, &ctx, TenantArgs::default()).await;
        let key = fixtures::issue_tenant_key(&ctx, tenant.tenant_id, tenant.owner_id).await;
        // Direct write (c): the insert has to fail for a reason other than a unique violation, which no valid request can cause, so the target table is dropped.
        let mut drop = sea_orm::sea_query::Table::drop();
        drop.table(sea_orm::sea_query::Alias::new("template_templates"));
        if ctx.db.get_database_backend() == DbBackend::Postgres {
            drop.cascade();
        }
        ctx.db.execute(&drop).await.unwrap();
        let response = request
            .post("/api/template-library")
            .add_header("Authorization", format!("Bearer {key}"))
            .json(&serde_json::json!({"name":"orphan","definition":note_definition()}))
            .await;
        response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !response.text().contains("already exists"),
            "{}",
            response.text()
        );
    })
    .await;
}
