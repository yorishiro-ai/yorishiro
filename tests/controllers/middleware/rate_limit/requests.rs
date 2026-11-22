use crate::requests::boot_request;
use axum::http::StatusCode;
use yorishiro::App;

#[tokio::test]
async fn auth_endpoints_are_rate_limited() {
    boot_request::<App, _, _>(|request, _ctx| async move {
        let mut last_status = 200;
        for _ in 0..15 {
            let response = request
                .post("/auth/root/login")
                .json(&serde_json::json!({ "email": "nobody@example.com", "password": "wrong" }))
                .await;
            last_status = response.status_code().as_u16();
        }
        assert_eq!(
            last_status, 429,
            "YORISHIRO_AUTH_RATE_LIMIT_MAX defaults to 10 per window; 15 attempts must exhaust it"
        );
        request
            .post("/auth/tenant/login")
            .add_header("x-tenant-id", uuid::Uuid::new_v4().to_string())
            .json(&serde_json::json!({"user_name":"nobody", "password":"wrong"}))
            .await
            .assert_status(StatusCode::TOO_MANY_REQUESTS);
    })
    .await;
}
