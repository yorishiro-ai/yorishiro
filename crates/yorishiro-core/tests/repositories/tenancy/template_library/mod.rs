use sqlx::PgPool;
use std::collections::BTreeMap;

use crate::YorishiroError;
use crate::metaschema::{EntityTypeDef, FieldDef, FieldTypeName, MetaSchemaDefinition};
use crate::repositories::tenancy::{
    CreateTemplateInput, UpdateTemplateInput, create_template, create_tenant, delete_template,
    fork_template, get_template, list_templates, update_template,
};

fn sample_definition(name: &str) -> MetaSchemaDefinition {
    let mut fields = BTreeMap::new();
    fields.insert(
        "title".to_string(),
        FieldDef {
            r#type: FieldTypeName::String,
            required: true,
            description: None,
            enum_values: None,
            format: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            min_items: None,
            max_items: None,
            unique_items: false,
            default: None,
            items: None,
            properties: None,
            x_embed: false,
            x_ui: None,
            extra: Default::default(),
        },
    );

    let mut entity_types = BTreeMap::new();
    entity_types.insert(
        "note".to_string(),
        EntityTypeDef {
            description: None,
            fields,
        },
    );

    MetaSchemaDefinition {
        name: name.to_string(),
        description: Some("a sample template".to_string()),
        entity_types,
        relation_types: Default::default(),
    }
}

fn sample_input(name: &str) -> CreateTemplateInput {
    CreateTemplateInput {
        name: name.to_string(),
        description: Some("a sample template".to_string()),
        definition: sample_definition(name),
        tags: vec!["notes".to_string(), "general".to_string()],
        locale: Some("en".to_string()),
        author: Some("Alice".to_string()),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn creates_and_lists_templates(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();

    let created = create_template(&pool, tenant.id, None, sample_input("my-template"))
        .await
        .unwrap();
    assert_eq!(created.tenant_id, tenant.id);
    assert_eq!(created.name, "my-template");
    assert_eq!(created.tags, vec!["notes", "general"]);
    assert_eq!(created.visibility, "tenant");

    let templates = list_templates(&pool, tenant.id).await.unwrap();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].id, created.id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_template_enforces_tenant_boundary(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "a", None).await.unwrap();
    let tenant_b = create_tenant(&pool, "b", None).await.unwrap();

    let created = create_template(&pool, tenant_a.id, None, sample_input("owned-by-a"))
        .await
        .unwrap();

    let fetched = get_template(&pool, tenant_a.id, created.id).await.unwrap();
    assert_eq!(fetched.id, created.id);

    let err = get_template(&pool, tenant_b.id, created.id)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_template_rejects_duplicate_name(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    create_template(&pool, tenant.id, None, sample_input("dup"))
        .await
        .unwrap();

    let err = create_template(&pool, tenant.id, None, sample_input("dup"))
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_template_rejects_invalid_definition(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();

    let mut input = sample_input("bad");
    input.definition.entity_types.clear();

    let err = create_template(&pool, tenant.id, None, input)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::ValidationFailed { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn updates_template_fields(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let created = create_template(&pool, tenant.id, None, sample_input("to-update"))
        .await
        .unwrap();

    let updated = update_template(
        &pool,
        tenant.id,
        created.id,
        UpdateTemplateInput {
            name: Some("renamed".to_string()),
            description: Some("new description".to_string()),
            definition: None,
            tags: Some(vec!["updated".to_string()]),
            locale: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.description, Some("new description".to_string()));
    assert_eq!(updated.tags, vec!["updated"]);
    assert_eq!(updated.locale, Some("en".to_string()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn update_template_rejects_other_tenant(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "a", None).await.unwrap();
    let tenant_b = create_tenant(&pool, "b", None).await.unwrap();
    let created = create_template(&pool, tenant_a.id, None, sample_input("owned-by-a"))
        .await
        .unwrap();

    let err = update_template(
        &pool,
        tenant_b.id,
        created.id,
        UpdateTemplateInput {
            name: Some("stolen".to_string()),
            description: None,
            definition: None,
            tags: None,
            locale: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn deletes_template_owned_by_tenant(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let created = create_template(&pool, tenant.id, None, sample_input("to-delete"))
        .await
        .unwrap();

    delete_template(&pool, tenant.id, created.id).await.unwrap();

    let templates = list_templates(&pool, tenant.id).await.unwrap();
    assert!(templates.is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_template_rejects_other_tenant(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "a", None).await.unwrap();
    let tenant_b = create_tenant(&pool, "b", None).await.unwrap();
    let created = create_template(&pool, tenant_a.id, None, sample_input("owned-by-a"))
        .await
        .unwrap();

    let err = delete_template(&pool, tenant_b.id, created.id)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}

#[sqlx::test(migrations = "../../migrations")]
async fn forks_template_within_the_same_tenant(pool: PgPool) {
    let tenant = create_tenant(&pool, "acme", None).await.unwrap();
    let created = create_template(&pool, tenant.id, None, sample_input("original"))
        .await
        .unwrap();

    let forked = fork_template(&pool, tenant.id, None, created.id, "forked".to_string())
        .await
        .unwrap();

    assert_eq!(forked.tenant_id, tenant.id);
    assert_eq!(forked.name, "forked");
    assert_eq!(forked.fork_of, Some(created.id));
    assert_eq!(forked.tags, created.tags);

    let templates = list_templates(&pool, tenant.id).await.unwrap();
    assert_eq!(templates.len(), 2);
}

/// A template with the default `visibility = 'tenant'` is invisible to other tenants, so
/// `fork_template` -- which resolves its source through `get_template` -- rejects forking
/// another tenant's private template.
#[sqlx::test(migrations = "../../migrations")]
async fn fork_template_rejects_other_tenants_private_template(pool: PgPool) {
    let tenant_a = create_tenant(&pool, "a", None).await.unwrap();
    let tenant_b = create_tenant(&pool, "b", None).await.unwrap();
    let created = create_template(&pool, tenant_a.id, None, sample_input("original"))
        .await
        .unwrap();

    let err = fork_template(&pool, tenant_b.id, None, created.id, "forked".to_string())
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::NotFound { .. }));
}
