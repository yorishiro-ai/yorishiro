use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::SystemTime;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Barrier;

use tempfile::tempdir;
use yorishiro::services::embedding::model_fetch::*;

struct FakeHttp {
    response: std::sync::Mutex<Option<Result<ArtifactHttpResponse, ArtifactHttpError>>>,
    urls: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl ArtifactHttp for FakeHttp {
    async fn get(
        &self,
        _client: &reqwest::Client,
        url: &str,
    ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
        self.urls.lock().unwrap().push(url.to_owned());
        self.response.lock().unwrap().take().unwrap()
    }
}

struct FakeBody {
    chunks: VecDeque<Result<Vec<u8>, ArtifactHttpError>>,
    on_next: Option<Arc<dyn Fn() + Send + Sync>>,
}

#[async_trait]
impl ArtifactBody for FakeBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError> {
        if let Some(on_next) = &self.on_next {
            on_next();
        }
        self.chunks.pop_front().transpose()
    }
}

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
        sha256: Box::leak(hex::encode(Sha256::digest(bytes)).into_boxed_str()),
        size: bytes.len() as u64,
        description: "test artifact",
    }
}

fn response(
    status: u16,
    chunks: impl IntoIterator<Item = Result<Vec<u8>, ArtifactHttpError>>,
) -> ArtifactHttpResponse {
    ArtifactHttpResponse {
        status: reqwest::StatusCode::from_u16(status).unwrap(),
        body: Box::new(FakeBody {
            chunks: chunks.into_iter().collect(),
            on_next: None,
        }),
    }
}

fn fake(response: Result<ArtifactHttpResponse, ArtifactHttpError>) -> FakeHttp {
    FakeHttp {
        response: std::sync::Mutex::new(Some(response)),
        urls: std::sync::Mutex::new(Vec::new()),
    }
}

fn assert_no_partials(dir: &Path) {
    assert!(!fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".partial.")
    }));
}

#[test]
fn configured_artifacts_use_lowercase_sha256_hex() {
    for artifact in [&DEFAULT_MODEL.model, &DEFAULT_MODEL.tokenizer] {
        assert_eq!(artifact.sha256.len(), 64, "{}", artifact.description);
        assert!(
            artifact
                .sha256
                .chars()
                .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && c <= 'f')),
            "{} digest must be lowercase hex",
            artifact.description
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
    fs::write(&stale, b"old").unwrap();
    fs::write(&fresh, b"live").unwrap();
    fs::write(&unrelated, b"other").unwrap();
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
async fn transport_failure_maps_timeout_and_cleans_staging() {
    let dir = tempdir().unwrap();
    let artifact = artifact(b"payload");
    let http = fake(Err(ArtifactHttpError::Timeout(
        "operation timed out".into(),
    )));

    let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("request timed out"), "{error}");
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn response_stream_timeout_keeps_timeout_mapping() {
    let dir = tempdir().unwrap();
    let artifact = artifact(b"payload");
    let http = fake(Ok(response(
        200,
        [Err(ArtifactHttpError::Timeout("body timed out".into()))],
    )));

    let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("request timed out"), "{error}");
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn non_success_status_does_not_expose_or_consume_response_body() {
    let dir = tempdir().unwrap();
    let consumed = Arc::new(AtomicBool::new(false));
    let body_consumed = Arc::clone(&consumed);
    let response = ArtifactHttpResponse {
        status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        body: Box::new(FakeBody {
            chunks: vec![Ok(b"secret upstream body".to_vec())]
                .into_iter()
                .collect(),
            on_next: Some(Arc::new(move || {
                body_consumed.store(true, Ordering::SeqCst);
            })),
        }),
    };
    let http = fake(Ok(response));
    let error = ensure_file_with_http(
        dir.path(),
        DEFAULT_MODEL,
        &artifact(b"payload"),
        &client(),
        &http,
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("HTTP 500"), "{error}");
    assert!(!error.contains("secret upstream body"), "{error}");
    assert!(!consumed.load(Ordering::SeqCst));
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn truncated_download_is_rejected_and_removed() {
    let dir = tempdir().unwrap();
    let artifact = artifact(b"complete payload");
    let http = fake(Ok(response(200, [Ok(b"truncated".to_vec())])));

    let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("downloaded 9 bytes, expected 16"), "{error}");
    assert!(!dir.path().join("test.bin").exists());
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn digest_mismatch_is_rejected_and_removed() {
    let dir = tempdir().unwrap();
    let artifact = Artifact {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        ..artifact(b"payload")
    };
    let http = fake(Ok(response(200, [Ok(b"payload".to_vec())])));

    let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap_err()
        .to_string();

    assert!(error.contains("SHA256"), "{error}");
    assert!(!dir.path().join("test.bin").exists());
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn stale_partials_are_removed_and_failed_partials_do_not_survive() {
    let dir = tempdir().unwrap();
    let stale = dir.path().join("test.bin.partial.old");
    fs::write(&stale, b"old partial").unwrap();
    fs::File::open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(1))
        .unwrap();

    let artifact = artifact(b"payload");
    let http = fake(Ok(response(200, [Ok(b"payload".to_vec())])));
    let path = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap();
    assert!(path.exists());
    assert!(!stale.exists());
    assert_no_partials(dir.path());

    fs::remove_file(path).unwrap();
    let failing = fake(Ok(response(
        200,
        [Err(ArtifactHttpError::Response("connection lost".into()))],
    )));
    let _ = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &failing).await;
    assert_no_partials(dir.path());
}

#[tokio::test]
async fn cache_hit_skips_transport_and_wrong_length_cache_is_replaced() {
    let dir = tempdir().unwrap();
    let bytes = b"payload";
    let artifact = artifact(bytes);
    let destination = dir.path().join(artifact.local_name);
    fs::write(&destination, bytes).unwrap();
    let cached = fake(Err(ArtifactHttpError::Request("must not be called".into())));
    assert_eq!(
        ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &cached,)
            .await
            .unwrap(),
        destination
    );
    assert!(cached.urls.lock().unwrap().is_empty());

    fs::write(&destination, b"bad").unwrap();
    let replacement = fake(Ok(response(200, [Ok(bytes.to_vec())])));
    ensure_file_with_http(
        dir.path(),
        DEFAULT_MODEL,
        &artifact,
        &client(),
        &replacement,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(&destination).unwrap(), bytes);
}

#[tokio::test]
async fn verified_bytes_rename_atomically_and_use_pinned_url() {
    let dir = tempdir().unwrap();
    let artifact = artifact(b"payload");
    let destination = dir.path().join(artifact.local_name);
    let checked_before_rename = Arc::new(AtomicBool::new(false));
    let checked = Arc::clone(&checked_before_rename);
    let destination_during_download = destination.clone();
    let partial_dir = dir.path().to_path_buf();
    let response = ArtifactHttpResponse {
        status: reqwest::StatusCode::OK,
        body: Box::new(FakeBody {
            chunks: vec![Ok(b"payload".to_vec())].into_iter().collect(),
            on_next: Some(Arc::new(move || {
                assert!(!destination_during_download.exists());
                assert_eq!(partial_files(&partial_dir).len(), 1);
                checked.store(true, Ordering::SeqCst);
            })),
        }),
    };
    let http = fake(Ok(response));
    ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
        .await
        .unwrap();

    assert!(checked_before_rename.load(Ordering::SeqCst));
    assert_eq!(fs::read(destination).unwrap(), b"payload");
    assert_eq!(
        http.urls.lock().unwrap().as_slice(),
        &[
            "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/test.bin"
        ]
    );
}

#[tokio::test]
async fn concurrent_fetches_use_distinct_staging_paths_and_leave_valid_bytes() {
    let dir = tempdir().unwrap();
    let bytes = b"payload";
    let artifact = artifact(bytes);
    let entered = Arc::new(Barrier::new(2));
    let inspected = Arc::new(Barrier::new(2));
    let http = Arc::new(ConcurrentHttp {
        dir: dir.path().to_path_buf(),
        entered: Arc::clone(&entered),
        inspected: Arc::clone(&inspected),
        bytes: bytes.to_vec(),
    });
    let client = client();

    let first = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client, http.as_ref());
    let second =
        ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client, http.as_ref());
    let (first, second) = tokio::join!(first, second);

    let successes = [first.as_ref(), second.as_ref()]
        .into_iter()
        .filter(|result| result.is_ok())
        .count();
    if cfg!(windows) {
        assert_eq!(
            successes, 1,
            "Windows cannot rename over an existing destination"
        );
    } else {
        assert_eq!(
            successes, 2,
            "same-filesystem rename should be atomic for both fetches"
        );
    }
    assert_eq!(
        fs::read(dir.path().join(artifact.local_name)).unwrap(),
        bytes
    );
    assert_no_partials(dir.path());
}

struct ConcurrentHttp {
    dir: PathBuf,
    entered: Arc<Barrier>,
    inspected: Arc<Barrier>,
    bytes: Vec<u8>,
}

#[async_trait]
impl ArtifactHttp for ConcurrentHttp {
    async fn get(
        &self,
        _client: &reqwest::Client,
        _url: &str,
    ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
        Ok(ArtifactHttpResponse {
            status: reqwest::StatusCode::OK,
            body: Box::new(ConcurrentBody {
                dir: self.dir.clone(),
                entered: Arc::clone(&self.entered),
                inspected: Arc::clone(&self.inspected),
                bytes: Some(self.bytes.clone()),
            }),
        })
    }
}

struct ConcurrentBody {
    dir: PathBuf,
    entered: Arc<Barrier>,
    inspected: Arc<Barrier>,
    bytes: Option<Vec<u8>>,
}

#[async_trait]
impl ArtifactBody for ConcurrentBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError> {
        self.entered.wait().await;
        assert_eq!(partial_files(&self.dir).len(), 2);
        self.inspected.wait().await;
        Ok(self.bytes.take())
    }
}

fn partial_files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".partial.")
        })
        .collect()
}

async fn loopback_server(response: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        loop {
            let read = socket.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        socket.write_all(&response).await.unwrap();
        request
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn reqwest_transport_sends_get_to_the_pinned_path_and_streams_body() {
    let response =
        b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nhello, world!";
    let (base, request) = loopback_server(response.to_vec()).await;
    let url = format!(
        "{base}/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/model.safetensors"
    );
    let response = ReqwestArtifactHttp.get(&client(), &url).await.unwrap();
    assert!(response.status.is_success());
    let mut body = response.body;
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next_chunk().await.unwrap() {
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(bytes, b"hello, world!");

    let request = String::from_utf8(request.await.unwrap()).unwrap();
    assert!(request.starts_with("GET /intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/model.safetensors HTTP/1.1\r\n"));
}

#[tokio::test]
async fn reqwest_transport_status_does_not_expose_response_body() {
    let response = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 21\r\nConnection: close\r\n\r\nsecret upstream body";
    let (base, request) = loopback_server(response.to_vec()).await;
    let url = format!("{base}/artifact");
    let temp = tempdir().unwrap();
    let artifact = artifact(b"payload");
    let error = download_verified(
        &url,
        &temp.path().join("artifact.partial"),
        &artifact,
        &client(),
        &ReqwestArtifactHttp,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("HTTP 503"), "{error}");
    assert!(!error.contains("secret upstream body"), "{error}");
    request.await.unwrap();
}

#[tokio::test]
async fn reqwest_transport_maps_request_and_stream_errors() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let request_error = ReqwestArtifactHttp
        .get(&client(), &format!("http://{address}/artifact"))
        .await
        .err()
        .expect("a closed loopback listener must reject the request");
    assert!(matches!(request_error, ArtifactHttpError::Request(_)));

    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nshort";
    let (base, request) = loopback_server(response.to_vec()).await;
    let mut response = ReqwestArtifactHttp
        .get(&client(), &format!("{base}/artifact"))
        .await
        .unwrap();
    let stream_error = loop {
        match response.body.next_chunk().await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("the truncated response must fail while streaming"),
            Err(error) => break error,
        }
    };
    assert!(matches!(stream_error, ArtifactHttpError::Response(_)));
    request.await.unwrap();
}

#[tokio::test]
async fn reqwest_transport_maps_request_timeout() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let short_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let error = ReqwestArtifactHttp
        .get(&short_client, &format!("http://{address}/artifact"))
        .await
        .err()
        .expect("the delayed response must time out");
    assert!(matches!(error, ArtifactHttpError::Timeout(_)));
}

#[tokio::test]
async fn reqwest_transport_maps_stream_timeout() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });
    let short_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let mut response = ReqwestArtifactHttp
        .get(&short_client, &format!("http://{address}/artifact"))
        .await
        .unwrap();
    let error = response.body.next_chunk().await.unwrap_err();
    assert!(matches!(error, ArtifactHttpError::Timeout(_)));
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
            .all(|c| { c.is_ascii_alphanumeric() || c == '-' || c == '.' })
    );
}
