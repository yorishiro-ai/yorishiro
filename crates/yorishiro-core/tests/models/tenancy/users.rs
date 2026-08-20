use sqlx::PgPool;

use crate::YorishiroError;
use crate::models::tenancy::{create_user, verify_login};

#[sqlx::test(migrations = "../../migrations")]
async fn creates_user_and_verifies_login(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let user = create_user(&mut conn, "alice@example.com", "hunter2", Some("Alice"))
        .await
        .unwrap();
    drop(conn);
    assert_eq!(user.email, "alice@example.com");

    let ok = verify_login(&pool, "alice@example.com", "hunter2")
        .await
        .unwrap();
    assert!(ok.is_some());

    let bad = verify_login(&pool, "alice@example.com", "wrong-password")
        .await
        .unwrap();
    assert!(bad.is_none());

    let unknown = verify_login(&pool, "nobody@example.com", "hunter2")
        .await
        .unwrap();
    assert!(unknown.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn rejects_duplicate_email(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    create_user(&mut conn, "bob@example.com", "pw", None)
        .await
        .unwrap();
    let err = create_user(&mut conn, "bob@example.com", "pw2", None)
        .await
        .unwrap_err();
    assert!(matches!(err, YorishiroError::Conflict { .. }));
}
