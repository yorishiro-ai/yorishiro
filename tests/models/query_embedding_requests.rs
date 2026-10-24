use loco_rs::app::AppContext;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait, sea_query::Expr};
use uuid::Uuid;
use yorishiro::App;
use yorishiro::models::query_embedding_requests::{
    Column, Entity, QueryEmbeddingStatus, QueryOutcome,
};

use crate::requests::fixtures::{TenantArgs, TestTenant};
use crate::requests::{boot_request, boot_request_isolated};

async fn workspace(request: &loco_rs::TestServer, ctx: &AppContext, name: &str) -> (Uuid, Uuid) {
    let fixture = TestTenant::create(
        request,
        ctx,
        TenantArgs {
            tenant_name: format!("{name}-{}", Uuid::new_v4().simple()),
            ..Default::default()
        },
    )
    .await;
    (fixture.tenant_id, fixture.workspace_id)
}

#[tokio::test]
async fn a_request_moves_from_pending_to_ready_and_is_consumed_once() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-ready").await;
        let id = Entity::open(
            &ctx.db,
            ws,
            "quarterly roadmap",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .expect("open");

        let row = Entity::find_pending(&ctx.db, ws, id)
            .await
            .expect("find")
            .expect("pending row");
        assert_eq!(row.query_text, "quarterly roadmap");
        assert_eq!(
            Entity::take(&ctx.db, ws, id).await.unwrap(),
            Some(QueryOutcome::Pending)
        );

        let vector = vec![0.25_f32, -1.5, 3.0e-3, 1.0e7];
        assert!(
            Entity::complete(&ctx.db, ws, id, &vector, "m")
                .await
                .unwrap()
        );
        assert!(
            Entity::find_pending(&ctx.db, ws, id)
                .await
                .unwrap()
                .is_none(),
            "a completed request is no longer pending"
        );
        assert_eq!(
            Entity::take(&ctx.db, ws, id).await.unwrap(),
            Some(QueryOutcome::Ready {
                vector,
                model: "m".into()
            })
        );
        assert_eq!(
            Entity::take(&ctx.db, ws, id).await.unwrap(),
            None,
            "a consumed result is gone"
        );
    })
    .await;
}

#[tokio::test]
async fn a_failed_request_reports_its_diagnostic_once() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-failed").await;
        let id = Entity::open(
            &ctx.db,
            ws,
            "q",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(
            Entity::fail(&ctx.db, ws, id, "provider is down")
                .await
                .unwrap()
        );
        assert!(
            !Entity::fail(&ctx.db, ws, id, "again").await.unwrap(),
            "a request fails once"
        );
        assert_eq!(
            Entity::take(&ctx.db, ws, id).await.unwrap(),
            Some(QueryOutcome::Failed("provider is down".into()))
        );
        assert_eq!(Entity::take(&ctx.db, ws, id).await.unwrap(), None);
    })
    .await;
}

#[tokio::test]
async fn a_late_result_cannot_revive_an_expired_or_consumed_request() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-late").await;

        let expired = Entity::open(
            &ctx.db,
            ws,
            "q",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(Entity::expire(&ctx.db, ws, expired).await.unwrap());
        assert!(
            !Entity::complete(&ctx.db, ws, expired, &[1.0], "m")
                .await
                .unwrap(),
            "the worker's late result is discarded"
        );
        assert!(!Entity::fail(&ctx.db, ws, expired, "late").await.unwrap());
        assert_eq!(
            Entity::take(&ctx.db, ws, expired).await.unwrap(),
            Some(QueryOutcome::Expired)
        );

        let consumed = Entity::open(
            &ctx.db,
            ws,
            "q",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(
            Entity::complete(&ctx.db, ws, consumed, &[1.0], "m")
                .await
                .unwrap()
        );
        assert!(
            !Entity::expire(&ctx.db, ws, consumed).await.unwrap(),
            "a finished request cannot be expired"
        );
        Entity::take(&ctx.db, ws, consumed).await.unwrap();
        assert!(
            !Entity::complete(&ctx.db, ws, consumed, &[2.0], "m")
                .await
                .unwrap(),
            "a consumed request cannot be completed again"
        );
        assert_eq!(Entity::take(&ctx.db, ws, consumed).await.unwrap(), None);
    })
    .await;
}

#[tokio::test]
async fn concurrent_consumers_share_one_result() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-race").await;
        let id = Entity::open(
            &ctx.db,
            ws,
            "q",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(
            Entity::complete(&ctx.db, ws, id, &[1.0, 2.0], "m")
                .await
                .unwrap()
        );

        let reads = (0..8).map(|_| {
            let db = ctx.db.clone();
            tokio::spawn(async move { Entity::take(&db, ws, id).await })
        });
        let mut ready = 0;
        for read in reads {
            match read.await.expect("join") {
                Ok(Some(QueryOutcome::Ready { .. })) => ready += 1,
                Ok(None) => {}
                other => panic!("unexpected consumer outcome: {other:?}"),
            }
        }
        assert_eq!(ready, 1, "exactly one consumer takes the result");
    })
    .await;
}

#[tokio::test]
async fn a_malformed_stored_result_is_an_internal_error_and_is_consumed() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-malformed").await;
        for stored in [
            serde_json::json!({"not": "a vector"}),
            serde_json::json!([1.0, 2.0, 3.0]),
        ] {
            let id = Entity::open(
                &ctx.db,
                ws,
                "q",
                60,
                chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
            assert!(
                Entity::complete(&ctx.db, ws, id, &[1.0, 2.0], "m")
                    .await
                    .unwrap()
            );
            // Corrupt only the stored vector; the second case disagrees with the persisted width.
            Entity::update_many()
                .col_expr(Column::ResultVector, Expr::value(stored))
                .filter(Column::Id.eq(id))
                .exec(&ctx.db)
                .await
                .unwrap();
            let error = Entity::take(&ctx.db, ws, id)
                .await
                .expect_err("malformed result");
            assert!(error.to_string().contains("malformed"), "{error}");
            assert_eq!(Entity::take(&ctx.db, ws, id).await.unwrap(), None);
        }
    })
    .await;
}

#[tokio::test]
async fn expired_rows_are_purged_and_live_rows_are_kept() {
    boot_request_isolated::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-purge").await;
        let stale = Entity::open(
            &ctx.db,
            ws,
            "stale",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        let live = Entity::open(
            &ctx.db,
            ws,
            "live",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        // Advance only this row's expiry clock to exercise purge without a wall-clock wait.
        Entity::update_many()
            .col_expr(
                Column::ExpiresAt,
                Expr::value((chrono::Utc::now() - chrono::Duration::seconds(5)).fixed_offset()),
            )
            .filter(Column::Id.eq(stale))
            .exec(&ctx.db)
            .await
            .unwrap();

        assert_eq!(Entity::purge_expired(&ctx.db).await.unwrap(), 1);
        assert_eq!(Entity::take(&ctx.db, ws, stale).await.unwrap(), None);
        assert_eq!(
            Entity::take(&ctx.db, ws, live).await.unwrap(),
            Some(QueryOutcome::Pending)
        );
        assert!(
            Entity::find_pending(&ctx.db, ws, stale)
                .await
                .unwrap()
                .is_none(),
            "an expired request is never handed to a worker"
        );
    })
    .await;
}

#[tokio::test]
async fn a_request_is_invisible_to_another_workspace() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (tenant, mine) = workspace(&request, &ctx, "qe-mine").await;
        let txn = ctx.db.begin().await.expect("begin workspace creation");
        let theirs = yorishiro::models::tenancy::create_workspace(
            &txn,
            tenant,
            &format!("other-workspace-{}", Uuid::new_v4()),
            None,
            None,
        )
        .await
        .expect("create separate workspace")
        .id;
        txn.commit().await.expect("commit workspace creation");
        let id = Entity::open(
            &ctx.db,
            mine,
            "private",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();

        assert!(
            Entity::find_pending(&ctx.db, theirs, id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !Entity::complete(&ctx.db, theirs, id, &[1.0], "m")
                .await
                .unwrap()
        );
        assert!(!Entity::expire(&ctx.db, theirs, id).await.unwrap());
        assert_eq!(Entity::take(&ctx.db, theirs, id).await.unwrap(), None);
        assert_eq!(
            Entity::take(&ctx.db, mine, id).await.unwrap(),
            Some(QueryOutcome::Pending)
        );
    })
    .await;
}

#[tokio::test]
async fn the_database_accepts_every_status_the_enum_defines_and_rejects_others() {
    boot_request::<App, _, _>(|request, ctx| async move {
        let (_, ws) = workspace(&request, &ctx, "qe-check").await;
        let id = Entity::open(
            &ctx.db,
            ws,
            "q",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        for status in QueryEmbeddingStatus::ALL {
            Entity::update_many()
                .col_expr(Column::Status, Expr::value(status.as_db_str()))
                .filter(Column::Id.eq(id))
                .exec(&ctx.db)
                .await
                .unwrap_or_else(|error| panic!("CHECK rejected {status}: {error}"));
        }
        assert!(
            Entity::update_many()
                .col_expr(Column::Status, Expr::value("bogus"))
                .filter(Column::Id.eq(id))
                .exec(&ctx.db)
                .await
                .is_err()
        );
    })
    .await;
}

/// PostgreSQL scopes the table by `app.current_workspace`, so the tenant-scoped role sees and changes only its own workspace's rows.
#[tokio::test]
#[cfg(feature = "enterprise")]
async fn postgres_row_level_security_scopes_requests_to_the_current_workspace() {
    // Row-level security exists only on PostgreSQL.
    if !crate::require_postgres_backend() {
        return;
    }
    boot_request::<App, _, _>(|request, ctx| async move {
        let db = ctx
            .shared_store
            .get::<yorishiro::db::DbHandle>()
            .expect("tenant pool on PostgreSQL");
        let (tenant_a, ws_a) = workspace(&request, &ctx, "qe-rls-a").await;
        let (tenant_b, ws_b) = workspace(&request, &ctx, "qe-rls-b").await;
        let b_id = Entity::open(
            &ctx.db,
            ws_b,
            "b's query",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();

        let txn = db.tenant.begin_for_workspace(tenant_a, ws_a).await.unwrap();
        let a_id = Entity::open(
            &txn,
            ws_a,
            "a's query",
            60,
            chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        assert!(
            Entity::find_by_id(b_id).one(&txn).await.unwrap().is_none(),
            "workspace A cannot read workspace B's request"
        );
        assert!(
            Entity::open(
                &txn,
                ws_b,
                "forged",
                60,
                chrono::Utc::now().fixed_offset() + chrono::Duration::minutes(1)
            )
            .await
            .is_err(),
            "workspace A cannot insert a row for workspace B"
        );
        txn.rollback().await.unwrap();

        let txn = db.tenant.begin_for_workspace(tenant_b, ws_b).await.unwrap();
        assert!(Entity::find_by_id(a_id).one(&txn).await.unwrap().is_none());
        assert!(
            Entity::find_by_id(b_id).one(&txn).await.unwrap().is_some(),
            "workspace B reads its own request"
        );
        assert!(!Entity::expire(&txn, ws_a, b_id).await.unwrap());
        txn.rollback().await.unwrap();
        crate::requests::close_app_pools(&ctx).await;
    })
    .await;
}
