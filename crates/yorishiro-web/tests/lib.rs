use crate::fallback_service;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use tower::ServiceExt;

async fn get(router: Router, uri: &str) -> Response {
    router
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn serves_index_html_at_root_from_embedded_assets() {
    let router = Router::new().fallback_service(fallback_service(None));

    let response = get(router, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/html"
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8(body.to_vec()).unwrap().contains("<html"));
}

#[tokio::test]
async fn serves_a_named_asset_with_the_right_content_type() {
    let router = Router::new().fallback_service(fallback_service(None));

    let response = get(router, "/app.js").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/javascript"
    );
}

#[tokio::test]
async fn missing_embedded_asset_is_404() {
    let router = Router::new().fallback_service(fallback_service(None));

    let response = get(router, "/does-not-exist.txt").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn override_dir_serves_from_disk_instead_of_embedded_assets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>from disk</html>").unwrap();
    let router = Router::new().fallback_service(fallback_service(Some(
        dir.path().to_str().unwrap().to_string(),
    )));

    let response = get(router, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), b"<html>from disk</html>");
}

#[tokio::test]
async fn override_dir_404s_on_a_missing_file_without_falling_back_to_embedded_assets() {
    let dir = tempfile::tempdir().unwrap();
    let router = Router::new().fallback_service(fallback_service(Some(
        dir.path().to_str().unwrap().to_string(),
    )));

    let response = get(router, "/").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn override_dir_rejects_path_traversal_outside_the_serve_root() {
    let outer = tempfile::tempdir().unwrap();
    std::fs::write(outer.path().join("secret.txt"), "TOP_SECRET_CONTENT").unwrap();

    let webroot = outer.path().join("webroot");
    std::fs::create_dir(&webroot).unwrap();
    std::fs::write(webroot.join("index.html"), "<html>ok</html>").unwrap();

    let router = Router::new().fallback_service(fallback_service(Some(
        webroot.to_str().unwrap().to_string(),
    )));

    let response = get(router, "/../secret.txt").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("TOP_SECRET_CONTENT"));
}
