mod vector_blob_tests {
    use yorishiro::db::sqlite_vec_blob;

    #[test]
    fn blob_is_little_endian_f32_in_order() {
        assert_eq!(
            sqlite_vec_blob(&[1.0, -2.0]),
            [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0]
        );
        assert!(sqlite_vec_blob(&[]).is_empty());
    }
}

mod sqlite_pool_tests {
    use yorishiro::db::require_min_sqlite_connections;

    /// An authenticated request holds one connection on its transaction and needs a second for `last_used_at`.
    #[test]
    fn a_sqlite_pool_needs_two_connections() {
        let error = require_min_sqlite_connections(1).unwrap_err();
        assert!(error.contains("at least 2"), "{error}");
        assert!(require_min_sqlite_connections(2).is_ok());
        assert!(require_min_sqlite_connections(100).is_ok());
    }
}

mod advisory_lock_tests {
    use sea_orm::TransactionTrait;
    use yorishiro::db::{AppContextBackend, lock_for_update, try_lock_for_update};

    use crate::requests::boot_request;

    /// PostgreSQL excludes a second transaction until the first ends; SQLite has one writer and no named locks, so the lock is a no-op that always succeeds.
    #[tokio::test]
    #[serial_test::serial(process_environment)]
    async fn a_named_lock_excludes_another_transaction_only_on_postgres() {
        boot_request::<yorishiro::App, _, _>(|_request, ctx| async move {
            let key = format!("lock-test-{}", uuid::Uuid::now_v7());
            let postgres = ctx.is_postgres();

            let first = ctx.db.begin().await.expect("first transaction");
            let second = ctx.db.begin().await.expect("second transaction");
            lock_for_update(&first, &key).await.expect("take the lock");

            assert_eq!(
                try_lock_for_update(&second, &key).await.expect("try"),
                !postgres,
                "the same key is held by the first transaction"
            );
            assert!(
                try_lock_for_update(&second, &format!("{key}-other"))
                    .await
                    .expect("try another key"),
                "a different key is never blocked"
            );

            first.commit().await.expect("release the lock");
            assert!(
                try_lock_for_update(&second, &key).await.expect("try again"),
                "the lock is free once the first transaction ends"
            );
            second.rollback().await.expect("rollback");
        })
        .await;
    }
}
