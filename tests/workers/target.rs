//! Worker targets: how a target is named and read back, and how it scopes optional concurrency limits.

use uuid::Uuid;
use yorishiro::workers::embedding_sync::WorkerClass;
use yorishiro::workers::target::{WorkerTarget, targets_for_start};

const TENANT_A: &str = "0198f3a0-0000-7000-8000-00000000000a";

fn tenant(id: &str) -> WorkerTarget {
    WorkerTarget::Tenant(Uuid::parse_str(id).unwrap())
}

#[test]
fn each_target_has_one_tag_class_and_concurrency_key() {
    assert_eq!(WorkerTarget::Shared.tag(), "worker-target:shared");
    assert_eq!(WorkerTarget::Shared.class(), WorkerClass::Shared);
    assert_eq!(WorkerTarget::Shared.concurrency_key(), "shared:shared");

    assert_eq!(WorkerTarget::Official.tag(), "worker-target:official");
    assert_eq!(WorkerTarget::Official.class(), WorkerClass::Official);
    assert_eq!(
        WorkerTarget::Official.concurrency_key(),
        "official:official"
    );

    let target = tenant(TENANT_A);
    assert_eq!(target.tag(), format!("worker-target:tenant:{TENANT_A}"));
    assert_eq!(target.class(), WorkerClass::TenantPrivate);
    assert_eq!(
        target.concurrency_key(),
        format!("tenant_private:tenant:{TENANT_A}")
    );
}

/// Redis keeps one queue per target so that a backlog for one target cannot hide the jobs of another from the first 1000 entries a worker scans.
#[test]
fn a_targets_queue_is_its_tag() {
    for target in [
        WorkerTarget::Shared,
        WorkerTarget::Official,
        tenant(TENANT_A),
    ] {
        assert_eq!(target.queue(), target.tag());
    }
}

#[test]
fn a_tag_reads_back_into_the_target_that_wrote_it() {
    for target in [
        WorkerTarget::Shared,
        WorkerTarget::Official,
        tenant(TENANT_A),
        WorkerTarget::Tenant(Uuid::now_v7()),
    ] {
        assert_eq!(WorkerTarget::parse_tag(&target.tag()), Ok(target));
    }
}

/// SQLite matches a tag with `LIKE`, which ignores case and treats `_` and `%` as wildcards.
/// A tag with either would match tags it does not equal there and nowhere else.
#[test]
fn a_tag_holds_no_character_sqlite_like_would_read_as_a_pattern() {
    for target in [
        WorkerTarget::Shared,
        WorkerTarget::Official,
        WorkerTarget::Tenant(Uuid::now_v7()),
    ] {
        let tag = target.tag();
        assert!(
            !tag.contains(['%', '_', '"']) && tag == tag.to_lowercase(),
            "{tag}"
        );
    }
}

#[test]
fn a_tag_that_names_no_target_is_refused() {
    let refused = [
        // Not a target tag at all.
        "",
        "shared",
        "query-embedding",
        "infer-fill",
        "worker-class:shared",
        "worker-class:tenant-private",
        // A target tag with the wrong name.
        "worker-target:",
        "worker-target:other",
        "worker-target:SHARED",
        "worker-target:Official",
        "worker-target:shared ",
        " worker-target:shared",
        "worker-target:shared:extra",
        "worker-target:tenant",
        "worker-target:tenant:",
        // A tenant id that is not a UUID.
        "worker-target:tenant:not-a-uuid",
        "worker-target:tenant:0198f3a0",
        // A UUID that is not in the form `Uuid::hyphenated` prints.
        "worker-target:tenant:0198F3A0-0000-7000-8000-00000000000A",
        "worker-target:tenant:0198f3a0-0000-7000-8000-00000000000A",
        "worker-target:tenant:0198f3a0000070008000000000000a",
        "worker-target:tenant:{0198f3a0-0000-7000-8000-00000000000a}",
        "worker-target:tenant:urn:uuid:0198f3a0-0000-7000-8000-00000000000a",
        "worker-target:tenant:0198f3a0-0000-7000-8000-00000000000a ",
    ];
    for tag in refused {
        let error = WorkerTarget::parse_tag(tag).expect_err(tag);
        let message = error.to_string();
        assert!(
            message.contains("invalid worker target") || tag.is_empty(),
            "{message}"
        );
        assert!(message.contains("worker-target:shared"), "{message}");
    }
}

/// A worker that names no target would take only untagged jobs, and every job Yorishiro enqueues is tagged, so it would sit idle; the start is refused instead.
#[test]
fn a_start_naming_no_target_is_refused() {
    let message = targets_for_start(&[]).unwrap_err().to_string();
    assert!(message.contains("names no target"), "{message}");
    assert!(message.contains("worker-target:shared"), "{message}");
}

#[test]
fn a_start_serves_each_target_it_names_once_in_the_order_given() {
    let tags = [
        WorkerTarget::Official.tag(),
        tenant(TENANT_A).tag(),
        WorkerTarget::Shared.tag(),
        WorkerTarget::Official.tag(),
    ];
    assert_eq!(
        targets_for_start(&tags),
        Ok(vec![
            WorkerTarget::Official,
            tenant(TENANT_A),
            WorkerTarget::Shared
        ])
    );
}

/// One bad tag refuses the whole start, even beside good ones: a process must not come up serving only part of what it was told to.
#[test]
fn a_start_naming_one_invalid_tag_is_refused() {
    let tags = [
        WorkerTarget::Shared.tag(),
        "worker-class:official".to_owned(),
    ];
    let error = targets_for_start(&tags).unwrap_err();
    assert!(error.to_string().contains("worker-class:official"));
}

/// A Redis worker polls only the queues its configuration names, so a process serving a tenant's target has to list that tenant's queue.
#[test]
fn a_redis_worker_polls_the_queue_of_each_target_it_serves() {
    use loco_rs::config::{QueueConfig, RedisQueueConfig};

    let mut config = crate::sqlite_fixture_config("sqlite::memory:");
    config.queue = Some(QueueConfig::Redis(RedisQueueConfig {
        uri: "redis://localhost:6379/15".into(),
        dangerously_flush: false,
        queues: None,
        num_workers: 1,
        reaper: None,
    }));
    let served = [WorkerTarget::Shared, tenant(TENANT_A)];
    let queues: Vec<String> = served.iter().map(|target| target.queue()).collect();
    yorishiro::data::config::serve_worker_queues(&mut config, &queues);
    // Serving the same targets again adds nothing.
    yorishiro::data::config::serve_worker_queues(&mut config, &queues);

    let Some(QueueConfig::Redis(redis)) = config.queue else {
        panic!("the queue stays Redis");
    };
    assert_eq!(
        redis.queues,
        Some(vec![
            "worker-target:shared".to_owned(),
            format!("worker-target:tenant:{TENANT_A}"),
        ])
    );
}

/// Target keys group a tenant's workspaces, while shared and official keys span the deployment; limits are opt-in.
mod concurrency {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
    use uuid::Uuid;
    use yorishiro::models::_entities::{tenant_tenants, workspace_workspaces};
    use yorishiro::models::queue_job_dispatch_outbox;
    use yorishiro::models::queue_job_lifecycles::{
        Admission, Column, Enqueue, Entity, LifecycleStatus, Model,
    };
    use yorishiro::models::workspace_workspaces::WORKSPACE_STATUS_ACTIVE;
    use yorishiro::workers::embedding_sync::WorkerClass;
    use yorishiro::workers::reindex::{self, ReindexArgs};

    use super::{TENANT_A, tenant};

    async fn workspace_of_new_tenant(ctx: &loco_rs::app::AppContext) -> (Uuid, Uuid) {
        let tenant = tenant_tenants::ActiveModel {
            name: Set(format!("target-test-{}", Uuid::now_v7())),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert tenant");
        (tenant.id, workspace_of(ctx, tenant.id).await)
    }

    async fn workspace_of(ctx: &loco_rs::app::AppContext, tenant_id: Uuid) -> Uuid {
        workspace_workspaces::ActiveModel {
            tenant_id: Set(tenant_id),
            name: Set(format!("target-test-{}", Uuid::now_v7())),
            status: Set(WORKSPACE_STATUS_ACTIVE.to_owned()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert workspace")
        .id
    }

    /// Dispatches one reindex job through the production dispatcher and returns the lifecycle row it recorded.
    async fn dispatch(
        ctx: &loco_rs::app::AppContext,
        workspace_id: Uuid,
        class: WorkerClass,
    ) -> Model {
        reindex::enqueue_for_class(
            ctx,
            ReindexArgs {
                lifecycle_id: None,
                workspace_id,
                worker_class: class,
                startup: false,
            },
        )
        .await
        .expect("dispatch a reindex job");
        Entity::find()
            .filter(Column::WorkspaceId.eq(workspace_id))
            .one(&ctx.db)
            .await
            .expect("read the lifecycle row")
            .expect("the dispatch recorded a lifecycle row")
    }

    async fn start(ctx: &loco_rs::app::AppContext, row: &Model) -> Admission {
        Entity::start_at(&ctx.db, row.id, chrono::Utc::now().fixed_offset())
            .await
            .expect("admit the job")
    }

    fn started(admission: &Admission) -> bool {
        matches!(admission, Admission::Started { .. })
    }

    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn tenant_private_jobs_keep_their_key_without_a_default_cap() {
        let guard = crate::EnvGuard::capture(&[
            "YORISHIRO_PLAN_POLICY_ENABLED",
            "YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT",
        ]);
        guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
        guard.set("YORISHIRO_WORKER_CLASS_TENANT_PRIVATE_LIMIT", "1");
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let (tenant_a, a_first) = workspace_of_new_tenant(&ctx).await;
            let a_second = workspace_of(&ctx, tenant_a).await;
            let (tenant_b, b_first) = workspace_of_new_tenant(&ctx).await;

            let first = dispatch(&ctx, a_first, WorkerClass::TenantPrivate).await;
            let second = dispatch(&ctx, a_second, WorkerClass::TenantPrivate).await;
            let other = dispatch(&ctx, b_first, WorkerClass::TenantPrivate).await;

            assert_eq!(
                first.concurrency_key.as_deref(),
                Some(format!("tenant_private:tenant:{tenant_a}").as_str())
            );
            assert_eq!(second.concurrency_key, first.concurrency_key);
            assert_eq!(
                other.concurrency_key.as_deref(),
                Some(format!("tenant_private:tenant:{tenant_b}").as_str())
            );
            assert_eq!(first.concurrency_limit, None);

            assert!(started(&start(&ctx, &first).await));
            assert!(started(&start(&ctx, &second).await));
            // Another tenant's job is not held back by it.
            assert!(started(&start(&ctx, &other).await));
        })
        .await;
    }

    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn shared_and_official_jobs_keep_their_keys_without_default_caps() {
        let guard = crate::EnvGuard::capture(&[
            "YORISHIRO_PLAN_POLICY_ENABLED",
            "YORISHIRO_WORKER_CLASS_SHARED_LIMIT",
            "YORISHIRO_WORKER_OFFICIAL_LIMIT",
        ]);
        guard.remove("YORISHIRO_PLAN_POLICY_ENABLED");
        guard.set("YORISHIRO_WORKER_CLASS_SHARED_LIMIT", "1");
        guard.set("YORISHIRO_WORKER_OFFICIAL_LIMIT", "1");
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            for (class, key) in [
                (WorkerClass::Shared, "shared:shared"),
                (WorkerClass::Official, "official:official"),
            ] {
                let (_, first_workspace) = workspace_of_new_tenant(&ctx).await;
                let (_, second_workspace) = workspace_of_new_tenant(&ctx).await;
                let first = dispatch(&ctx, first_workspace, class).await;
                let second = dispatch(&ctx, second_workspace, class).await;

                assert_eq!(first.concurrency_key.as_deref(), Some(key));
                assert_eq!(second.concurrency_key.as_deref(), Some(key));
                assert_eq!(
                    first.concurrency_limit, None,
                    "the default {key} cap is disabled"
                );

                assert!(started(&start(&ctx, &first).await), "{key}");
                assert!(started(&start(&ctx, &second).await), "{key}");
            }
        })
        .await;
    }

    /// The plan a workspace's tenant is on never enters the key, so it cannot split the deployment-wide limit into one per plan.
    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn the_plan_label_is_recorded_but_is_not_part_of_the_key() {
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let (_, workspace) = workspace_of_new_tenant(&ctx).await;
            let row = dispatch(&ctx, workspace, WorkerClass::Official).await;
            assert!(row.plan.is_some(), "the plan label stays on the row");
            let key = row.concurrency_key.expect("a concurrency key");
            assert_eq!(key, "official:official");
        })
        .await;
    }

    /// A tenant-private job for a workspace that no longer exists has no target, so it is refused and recorded rather than sent to some other worker.
    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn a_tenant_private_job_for_an_unknown_workspace_is_refused_and_recorded() {
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let missing = Uuid::now_v7();
            let error = reindex::enqueue_for_class(
                &ctx,
                ReindexArgs {
                    lifecycle_id: None,
                    workspace_id: missing,
                    worker_class: WorkerClass::TenantPrivate,
                    startup: false,
                },
            )
            .await
            .expect_err("no tenant owns the workspace");
            assert!(error.to_string().contains("worker target lookup failed"));
            let row = Entity::find()
                .filter(Column::WorkspaceId.eq(missing))
                .one(&ctx.db)
                .await
                .unwrap()
                .expect("the refusal is recorded");
            assert_eq!(row.status, LifecycleStatus::Unavailable.as_db_str());
        })
        .await;
    }

    /// Unfinished dispatches from a release that routed by worker class stop a worker from starting, and finished ones do not.
    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn unfinished_dispatches_from_the_class_routed_release_are_counted() {
        crate::requests::boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            assert_eq!(
                queue_job_dispatch_outbox::count_retired_routes(&ctx.db)
                    .await
                    .unwrap(),
                0
            );
            let mut recorded = Vec::new();
            for (name, tags) in [
                ("retired-class-tag", Some(vec!["worker-class:shared".to_owned()])),
                ("retired-job-tag", Some(vec!["infer-fill".to_owned()])),
                ("current", Some(vec![tenant(TENANT_A).tag()])),
                ("untagged", None),
            ] {
                let id = Uuid::now_v7();
                queue_job_dispatch_outbox::record(
                    &ctx.db,
                    Enqueue {
                        id,
                        job_name: "retired-route-test",
                        worker_class: WorkerClass::Shared,
                        workspace_id: None,
                        plan: None,
                        concurrency_key: None,
                        concurrency_limit: None,
                    },
                    serde_json::json!({}),
                    name.to_owned(),
                    None,
                    tags,
                )
                .await
                .unwrap();
                recorded.push((name, id));
            }
            assert_eq!(
                queue_job_dispatch_outbox::count_retired_routes(&ctx.db)
                    .await
                    .unwrap(),
                2,
                "only the two routes without a worker-target tag"
            );

            for status in ["queued", "running", "retrying", "completed", "failed", "cancelled"] {
                for (name, id) in &recorded {
                    if name.starts_with("retired") {
                        let row = Entity::find_by_id(*id).one(&ctx.db).await.unwrap().unwrap();
                        let mut row: yorishiro::models::_entities::queue_job_lifecycles::ActiveModel = row.into();
                        row.status = Set(status.to_owned());
                        row.update(&ctx.db).await.unwrap();
                    }
                }
                let unfinished = matches!(status, "queued" | "running" | "retrying");
                assert_eq!(
                    queue_job_dispatch_outbox::count_retired_routes(&ctx.db).await.unwrap(),
                    if unfinished { 2 } else { 0 },
                    "{status}"
                );
                if unfinished {
                    let queue = ctx.queue_provider.as_ref().expect("queue provider");
                    let error = <yorishiro::App as loco_rs::app::Hooks>::connect_workers(&ctx, queue)
                        .await
                        .expect_err("unfinished retired routes refuse worker startup");
                    assert!(error.to_string().contains("drain the queue"), "{error}");
                }
            }

            // Finishing a dispatch removes its outbox row, which is how a drained queue stops being counted.
            for (name, id) in recorded {
                if name.starts_with("retired") {
                    Entity::finish(&ctx.db, id, None, LifecycleStatus::Cancelled, None)
                        .await
                        .unwrap();
                }
            }
            assert_eq!(
                queue_job_dispatch_outbox::count_retired_routes(&ctx.db)
                    .await
                    .unwrap(),
                0
            );
        })
        .await;
    }
}
