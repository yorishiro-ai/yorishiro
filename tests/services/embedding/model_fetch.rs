use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tempfile::tempdir;
use yorishiro::services::embedding::model_fetch::*;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .unwrap()
}

fn artifact(bytes: &[u8]) -> Artifact {
    Artifact {
        remote_path: "test.bin",
        local_name: "test.bin",
        sha256: Box::leak(
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .into_boxed_str(),
        ),
        size: bytes.len() as u64,
        description: "test artifact",
    }
}

fn partial_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".partial.")
        })
        .collect()
}

fn assert_no_partials(dir: &Path) {
    assert!(partial_files(dir).is_empty());
}

async fn loopback_server(
    response: &[u8],
) -> (
    String,
    crate::http_stub::HttpStub,
    tokio::sync::oneshot::Receiver<Vec<u8>>,
) {
    let (stub, request) =
        crate::http_stub::HttpStub::raw_with_request(String::from_utf8(response.to_vec()).unwrap())
            .await;
    (format!("{}/artifact", stub.url), stub, request)
}

#[test]
fn configured_artifacts_use_lowercase_sha256_hex() {
    for artifact in [&DEFAULT_MODEL.model, &DEFAULT_MODEL.tokenizer] {
        assert_eq!(artifact.sha256.len(), 64, "{}", artifact.description);
        assert!(
            artifact
                .sha256
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
    }
    assert_ne!(DEFAULT_MODEL.model.sha256, DEFAULT_MODEL.tokenizer.sha256);
}

#[test]
fn sweep_removes_only_stale_partials_for_one_artifact() {
    let dir = tempdir().unwrap();
    let stale = dir.path().join("test.bin.partial.old");
    let fresh = dir.path().join("test.bin.partial.live");
    let unrelated = dir.path().join("other.bin.partial.old");
    for path in [&stale, &fresh, &unrelated] {
        fs::write(path, b"partial").unwrap();
    }
    fs::File::open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(1))
        .unwrap();
    sweep_stale_partials(dir.path(), &artifact(b"payload"));
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(unrelated.exists());
}

#[tokio::test]
async fn non_success_status_does_not_expose_or_wait_for_response_body() {
    let dir = tempdir().unwrap();
    let server = crate::http_stub::HttpStub::stalled_after_response(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 1000\r\n\r\nsecret upstream body",
    )
    .await;
    let url = format!("{}/artifact", server.url);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        ensure_file_with_client(dir.path(), &url, &artifact(b"payload"), &client()),
    )
    .await;
    drop(server);
    let error = result
        .expect("error status must not wait for the remaining body")
        .unwrap_err()
        .to_string();
    assert!(error.contains("HTTP 503"), "{error}");
    assert!(!error.contains("secret upstream body"), "{error}");
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn truncated_download_is_rejected_and_removed() {
    let dir = tempdir().unwrap();
    let (url, _stub, server) = loopback_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\ntruncated",
    )
    .await;
    let error =
        ensure_file_with_client(dir.path(), &url, &artifact(b"complete payload"), &client())
            .await
            .unwrap_err()
            .to_string();
    assert!(error.contains("downloaded 9 bytes, expected 16"), "{error}");
    assert!(!dir.path().join("test.bin").exists());
    assert_no_partials(dir.path());
    server.await.unwrap();
}

#[tokio::test]
async fn digest_mismatch_is_rejected_and_removed() {
    let dir = tempdir().unwrap();
    let artifact = Artifact {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        ..artifact(b"payload")
    };
    let (url, _stub, server) = loopback_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
    )
    .await;
    let error = ensure_file_with_client(dir.path(), &url, &artifact, &client())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("SHA256"), "{error}");
    assert!(!dir.path().join("test.bin").exists());
    assert_no_partials(dir.path());
    server.await.unwrap();
}

#[tokio::test]
async fn verified_download_sweeps_stale_partials_and_sends_get() {
    let dir = tempdir().unwrap();
    let stale = dir.path().join("test.bin.partial.old");
    fs::write(&stale, b"old partial").unwrap();
    fs::File::open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(1))
        .unwrap();
    let (url, _stub, server) = loopback_server(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\npay\r\n4\r\nload\r\n0\r\n\r\n").await;
    let path = ensure_file_with_client(dir.path(), &url, &artifact(b"payload"), &client())
        .await
        .unwrap();
    assert_eq!(fs::read(path).unwrap(), b"payload");
    assert!(!stale.exists());
    assert_no_partials(dir.path());
    assert!(
        String::from_utf8(server.await.unwrap())
            .unwrap()
            .starts_with("GET /artifact HTTP/1.1\r\n")
    );
}

#[tokio::test]
async fn cache_hit_skips_transport_and_wrong_length_cache_is_replaced() {
    let dir = tempdir().unwrap();
    let artifact = artifact(b"payload");
    let destination = dir.path().join(artifact.local_name);
    fs::write(&destination, b"payload").unwrap();
    // An invalid URL would fail immediately if a cache hit tried to fetch it.
    assert_eq!(
        ensure_file_with_client(dir.path(), "not a URL", &artifact, &client())
            .await
            .unwrap(),
        destination
    );
    fs::write(&destination, b"bad").unwrap();
    let (url, _stub, server) = loopback_server(
        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
    )
    .await;
    ensure_file_with_client(dir.path(), &url, &artifact, &client())
        .await
        .unwrap();
    assert_eq!(fs::read(destination).unwrap(), b"payload");
    assert_no_partials(dir.path());
    server.await.unwrap();
}

#[tokio::test]
async fn concurrent_fetches_use_distinct_staging_paths_and_rename_only_verified_bytes() {
    let dir = tempdir().unwrap();
    let partial_dir = dir.path().to_path_buf();
    let staged = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let server = crate::http_stub::HttpStub::start(axum::Router::new().route(
        "/artifact",
        axum::routing::get(move || {
            let partial_dir = partial_dir.clone();
            let staged = staged.clone();
            async move {
                let body = futures::stream::once(async move {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while partial_files(&partial_dir).len() != 2 {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        assert!(!partial_dir.join("test.bin").exists());
                        staged.wait().await;
                    })
                    .await
                    .expect("both clients must stage before either body is sent");
                    Ok::<_, std::io::Error>("payload")
                });
                (
                    [(axum::http::header::CONTENT_LENGTH, "7")],
                    axum::body::Body::from_stream(body),
                )
            }
        }),
    ))
    .await;
    let url = format!("{}/artifact", server.url);
    let artifact = artifact(b"payload");
    let client = client();
    let (first, second) = tokio::join!(
        ensure_file_with_client(dir.path(), &url, &artifact, &client),
        ensure_file_with_client(dir.path(), &url, &artifact, &client),
    );
    drop(server);
    let successes = [first, second]
        .iter()
        .filter(|result| result.is_ok())
        .count();
    assert_eq!(successes, if cfg!(windows) { 1 } else { 2 });
    assert_eq!(
        fs::read(dir.path().join(artifact.local_name)).unwrap(),
        b"payload"
    );
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn request_and_stream_errors_clean_staging() {
    let dir = tempdir().unwrap();
    let error = ensure_file_with_client(dir.path(), "not a URL", &artifact(b"payload"), &client())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("request failed"), "{error}");
    assert_no_partials(dir.path());
    let (url, _stub, server) =
        loopback_server(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nshort")
            .await;
    let error = ensure_file_with_client(dir.path(), &url, &artifact(b"payload"), &client())
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("response stream failed"), "{error}");
    assert_no_partials(dir.path());
    server.await.unwrap();
}

#[tokio::test]
async fn request_and_stream_timeouts_keep_timeout_mapping_and_clean_staging() {
    for headers in [false, true] {
        let dir = tempdir().unwrap();
        let response = if headers {
            "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n"
        } else {
            ""
        };
        let server = crate::http_stub::HttpStub::stalled_after_response(response).await;
        let url = format!("{}/artifact", server.url);
        let short_client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            ensure_file_with_client(dir.path(), &url, &artifact(b"payload"), &short_client),
        )
        .await;
        drop(server);
        let error = error
            .expect("reqwest read timeout must fire")
            .unwrap_err()
            .to_string();
        assert!(error.contains("request timed out"), "{error}");
        assert_eq!(
            error.contains("failed while downloading"),
            headers,
            "{error}"
        );
        assert_no_partials(dir.path());
    }
}

#[test]
fn artifact_definitions_pin_full_revisions_and_safe_urls() {
    assert_eq!(DEFAULT_MODEL.revision.len(), 40);
    assert!(
        DEFAULT_MODEL
            .revision
            .chars()
            .all(|c| c.is_ascii_hexdigit())
    );
    assert!(
        DEFAULT_MODEL
            .short_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    );
}
