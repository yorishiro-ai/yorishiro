use super::helpers;
use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::DbBackend;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    fn use_transaction(&self) -> Option<bool> {
        helpers::use_transaction()
    }

    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("queue_publishing_state"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .integer()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("epoch")).big_integer().not_null())
                    .col(ColumnDef::new(Alias::new("active")).boolean().not_null())
                    .col(helpers::ts_tz_col(manager, Alias::new("cutoff_at")))
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO queue_publishing_state (id, epoch, active) VALUES (1, 1, TRUE) ON CONFLICT (id) DO NOTHING",
            )
            .await?;
        let backend = manager.get_database_backend();
        if backend != DbBackend::Sqlite {
            manager.get_connection().execute_unprepared("ALTER TABLE queue_publishing_state ENABLE ROW LEVEL SECURITY; CREATE POLICY queue_publishing_state_visible ON queue_publishing_state USING (true) WITH CHECK (false);").await?;
            helpers::grant(manager, "SELECT", "queue_publishing_state").await?;
            manager.get_connection().execute_unprepared("CREATE FUNCTION public.queue_publishing_epoch_shared_lock() RETURNS bigint LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public AS $$ SELECT epoch FROM public.queue_publishing_state WHERE id = 1 FOR SHARE $$; REVOKE ALL ON FUNCTION public.queue_publishing_epoch_shared_lock() FROM PUBLIC; GRANT EXECUTE ON FUNCTION public.queue_publishing_epoch_shared_lock() TO yorishiro_app; CREATE FUNCTION public.queue_publishing_epoch_exclusive_lock() RETURNS bigint LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, public AS $$ SELECT epoch FROM public.queue_publishing_state WHERE id = 1 FOR UPDATE $$; REVOKE ALL ON FUNCTION public.queue_publishing_epoch_exclusive_lock() FROM PUBLIC").await?;
            helpers::grant(manager, "INSERT", "inference_jobs").await?;
        }
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "queue_job_dispatch_outbox",
        )
        .await?;
        let mut id = ColumnDef::new(Alias::new("id"));
        let mut lifecycle_id = ColumnDef::new(Alias::new("lifecycle_id"));
        let mut workspace_id = ColumnDef::new(Alias::new("workspace_id"));
        if backend == DbBackend::Sqlite {
            id.blob();
            lifecycle_id.blob();
            workspace_id.blob();
        } else {
            id.uuid();
            lifecycle_id.uuid();
            workspace_id.uuid();
        }
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("queue_job_dispatch_outbox_new"))
                    .col(id.not_null().primary_key())
                    .col(lifecycle_id.null())
                    .col(helpers::json_col(manager, Alias::new("payload")).not_null())
                    .col(
                        ColumnDef::new(Alias::new("worker_name"))
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Alias::new("queue_name")).string())
                    .col(helpers::json_col(manager, Alias::new("tags")))
                    .col(helpers::ts_tz_col(manager, Alias::new("last_attempt_at")))
                    .col(
                        ColumnDef::new(Alias::new("publishing_epoch"))
                            .big_integer()
                            .not_null(),
                    )
                    .col(helpers::ts_tz_col(manager, Alias::new("deadline_at")))
                    .col(helpers::ts_tz_col(manager, Alias::new("published_at")))
                    .col(ColumnDef::new(Alias::new("claim_token")).string())
                    .col(helpers::ts_tz_col(manager, Alias::new("claim_until")))
                    .col(ColumnDef::new(Alias::new("priority")).integer())
                    .col(workspace_id)
                    .col(
                        ColumnDef::new(Alias::new("legacy_quarantined"))
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        let insert = if backend == DbBackend::Sqlite {
            "INSERT INTO queue_job_dispatch_outbox_new (id, lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at, publishing_epoch, deadline_at, published_at, claim_token, claim_until, priority, workspace_id, legacy_quarantined) SELECT COALESCE(lifecycle_id, randomblob(16)), lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at, 1, NULL, NULL, NULL, NULL, NULL, (SELECT workspace_id FROM queue_job_lifecycles l WHERE l.id = queue_job_dispatch_outbox.lifecycle_id), CASE WHEN lifecycle_id IS NULL THEN 1 ELSE 0 END FROM queue_job_dispatch_outbox"
        } else {
            "INSERT INTO queue_job_dispatch_outbox_new (id, lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at, publishing_epoch, deadline_at, published_at, claim_token, claim_until, priority, workspace_id, legacy_quarantined) SELECT COALESCE(o.lifecycle_id, uuidv7()), o.lifecycle_id, o.payload, o.worker_name, o.queue_name, o.tags, o.last_attempt_at, 1, NULL, NULL, NULL, NULL, NULL, l.workspace_id, (o.lifecycle_id IS NULL) FROM queue_job_dispatch_outbox o LEFT JOIN queue_job_lifecycles l ON l.id = o.lifecycle_id"
        };
        manager.get_connection().execute_unprepared(insert).await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .to_owned(),
            )
            .await?;
        manager
            .rename_table(
                Table::rename()
                    .table(
                        Alias::new("queue_job_dispatch_outbox_new"),
                        Alias::new("queue_job_dispatch_outbox"),
                    )
                    .to_owned(),
            )
            .await?;
        helpers::grant(
            manager,
            "SELECT, INSERT, UPDATE, DELETE",
            "queue_job_dispatch_outbox",
        )
        .await?;
        helpers::enable_rls_with_policy(
            manager,
            "queue_job_dispatch_outbox",
            "workspace_isolation",
            "workspace_id",
            "app.current_workspace",
            true,
        )
        .await?;
        if backend != DbBackend::Sqlite {
            manager.get_connection().execute_unprepared(
                "ALTER TABLE queue_job_lifecycles ENABLE ROW LEVEL SECURITY; GRANT SELECT, INSERT, UPDATE ON queue_job_lifecycles TO yorishiro_app; CREATE POLICY workspace_lifecycle_select ON queue_job_lifecycles AS PERMISSIVE FOR SELECT USING (workspace_id IS NOT NULL AND workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid); CREATE POLICY workspace_lifecycle_insert ON queue_job_lifecycles AS PERMISSIVE FOR INSERT WITH CHECK (workspace_id IS NOT NULL AND workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid); CREATE POLICY workspace_lifecycle_update ON queue_job_lifecycles AS PERMISSIVE FOR UPDATE USING (workspace_id IS NOT NULL AND workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid) WITH CHECK (workspace_id IS NOT NULL AND workspace_id = NULLIF(current_setting('app.current_workspace', true), '')::uuid);",
            ).await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("queue_job_dispatch_outbox_lifecycle_idx")
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .col(Alias::new("lifecycle_id"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.get_database_backend() != DbBackend::Sqlite {
            manager.get_connection().execute_unprepared(
                "DROP POLICY IF EXISTS workspace_lifecycle_select ON queue_job_lifecycles; DROP POLICY IF EXISTS workspace_lifecycle_insert ON queue_job_lifecycles; DROP POLICY IF EXISTS workspace_lifecycle_update ON queue_job_lifecycles; ALTER TABLE queue_job_lifecycles DISABLE ROW LEVEL SECURITY; REVOKE SELECT, INSERT, UPDATE ON queue_job_lifecycles FROM yorishiro_app;",
            ).await?;
        }
        manager
            .drop_index(
                Index::drop()
                    .name("queue_job_dispatch_outbox_lifecycle_idx")
                    .table(Alias::new("queue_job_dispatch_outbox"))
                    .to_owned(),
            )
            .await?;
        if manager.get_database_backend() != DbBackend::Sqlite {
            manager.get_connection().execute_unprepared("ALTER TABLE queue_job_dispatch_outbox DISABLE ROW LEVEL SECURITY; REVOKE ALL ON queue_job_dispatch_outbox FROM yorishiro_app").await?;
            manager
                .get_connection()
                .execute_unprepared("REVOKE INSERT ON inference_jobs FROM yorishiro_app")
                .await?;
            manager.get_connection().execute_unprepared("DROP FUNCTION IF EXISTS public.queue_publishing_epoch_shared_lock(); DROP FUNCTION IF EXISTS public.queue_publishing_epoch_exclusive_lock();").await?;
        }
        if manager.get_database_backend() == DbBackend::Sqlite {
            manager
                .get_connection()
                .execute_unprepared(
                    "CREATE TABLE queue_job_dispatch_outbox_old (lifecycle_id BLOB PRIMARY KEY, payload TEXT NOT NULL, worker_name TEXT NOT NULL, queue_name TEXT, tags TEXT, last_attempt_at TEXT); INSERT INTO queue_job_dispatch_outbox_old SELECT lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at FROM (SELECT *, row_number() OVER (PARTITION BY lifecycle_id ORDER BY id) AS rn FROM queue_job_dispatch_outbox) WHERE rn = 1; DROP TABLE queue_job_dispatch_outbox; ALTER TABLE queue_job_dispatch_outbox_old RENAME TO queue_job_dispatch_outbox;",
                )
                .await?;
        } else {
            manager.get_connection().execute_unprepared("CREATE TABLE queue_job_dispatch_outbox_old (lifecycle_id UUID PRIMARY KEY, payload JSONB NOT NULL, worker_name TEXT NOT NULL, queue_name TEXT, tags JSONB, last_attempt_at TIMESTAMPTZ); INSERT INTO queue_job_dispatch_outbox_old SELECT lifecycle_id, payload, worker_name, queue_name, tags, last_attempt_at FROM (SELECT *, row_number() OVER (PARTITION BY lifecycle_id ORDER BY id) AS rn FROM queue_job_dispatch_outbox) q WHERE rn = 1; DROP TABLE queue_job_dispatch_outbox; ALTER TABLE queue_job_dispatch_outbox_old RENAME TO queue_job_dispatch_outbox;").await?;
        }
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("queue_publishing_state"))
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
