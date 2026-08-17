use super::{Assets, fallback_service};
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

/// The build gate.
/// `ee/web/dist` is produced by `pnpm run build` and is not committed, so a checkout that skipped that step embeds only `.gitkeep`, and every other test here still passes, because an empty embed serves 404s that look like ordinary misses.
/// This is the one that fails, rather than shipping a binary whose UI is a blank 404.
#[test]
fn embeds_a_built_spa() {
    assert!(
        Assets::get("index.html").is_some(),
        "ee/web/dist holds no index.html: run `pnpm run build` in ee/web before building"
    );
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

/// rsbuild hashes its output filenames, so no asset can be named literally.
/// The name is taken from the embed itself; what is asserted is that a real asset is served with the content type its extension implies, not that a particular file exists.
#[tokio::test]
async fn serves_a_named_asset_with_the_right_content_type() {
    let js = Assets::iter()
        .find(|p| p.ends_with(".js"))
        .expect("a built SPA has at least one .js asset");
    let router = Router::new().fallback_service(fallback_service(None));

    let response = get(router, &format!("/{js}")).await;

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
