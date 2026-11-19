//! CLI tasks run through `cargo loco task`: maintenance, tenant retirement, embedding resync and credential recovery.

// Tenant retirement: inspect, disable, delete.
pub(crate) mod delete_due_tenants;
pub mod delete_tenant;
pub mod disable_tenant;
pub mod inspect_tenant_retirement;

// Accounts and credentials.
pub(crate) mod delete_user;
pub(crate) mod reset_root_password;

// Embeddings and the queue.
pub(crate) mod embedding_index_maintenance;
pub(crate) mod recover_worker_dispatches;
pub(crate) mod resync_embeddings;
pub mod tenant_reindex_scheduler;

// Database and system maintenance.
pub(crate) mod db_load_guard;
pub mod maintenance;
pub(crate) mod maintenance_status;
