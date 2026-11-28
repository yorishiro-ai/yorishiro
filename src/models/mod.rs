//! One module per table, beside the generated entities in [`_entities`].
//!
//! Every generated entity needs an `ActiveModelBehavior` implementation in every build, and the ones that hold a secret need a redacting `Debug` (`scripts/harden_generated_entities.py` removes the derived one).
//! So a table gets its module here even when only an overlay's code reads and writes it: the schema is one set of migrations, one set of entities, and the behaviour that overlay owns stays in that overlay's own module for the table.

// Shared plumbing: the generated entities, the `db_enum!` conversions and pagination.
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::restriction,
    reason = "SeaORM-generated code is replaced by make entities"
)]
pub mod _entities;
pub mod db_enum;
pub mod pagination;

// Tenants, users, workspaces and their memberships and keys.
pub mod tenancy;
pub mod tenant_memberships;
pub mod tenant_retirement_events;
pub mod tenant_tenants;
pub mod user_users;
pub mod workspace_embedding_keys;
pub mod workspace_llm_keys;
pub mod workspace_worker_classes;
pub mod workspace_workspaces;

// Access keys.
pub mod access_key_audit_log;
pub mod access_keys;

// The append-only trail of every call.
pub mod api_events;

// IAM: policies, groups, roles and the checks that evaluate them.
pub mod iam;
pub mod iam_group_memberships;
pub mod iam_group_policy_attachments;
pub mod iam_groups;
pub mod iam_policies;
pub mod iam_policy_versions;
pub mod iam_tenant_roots;
pub mod iam_user_policy_attachments;

// Schemas, forks and the template library.
pub mod schema_schemas;
pub mod template_reviews;
pub mod template_templates;
pub mod template_versions;
pub mod workspace_schema_fork_heads;
pub mod workspace_schema_forks;

// Entities, relations, search and the embeddings behind it.
pub mod entity_column_preferences;
pub mod entity_embeddings;
pub mod entity_entities;
pub mod entity_relations;
pub mod entity_snapshots;
pub mod inference_jobs;
pub(crate) mod inference_proposals;
pub mod query_embedding_requests;
pub mod recall;
pub mod search;

// Workspace JSONL import and export.
pub mod transfer;

// Queue bookkeeping and system state.
pub mod queue_job_dispatch_outbox;
pub mod queue_job_lifecycles;
pub mod system_maintenance;

// Billing.
pub mod compute_credit_ledger;
pub mod stripe_events;
pub mod tenant_billing;
