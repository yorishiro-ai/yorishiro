use std::sync::{Arc, Mutex};

use yorishiro::services::embedding::EmbeddingProvider;

use axum::Router;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

/// Owns an external HTTP boundary for one test, including panic-safe shutdown.
pub(crate) struct HttpStub {
    pub(crate) url: String,
    task: JoinHandle<()>,
}

impl HttpStub {
    pub(crate) async fn stalled() -> Self {
        Self::stalled_after_response("").await
    }

    pub(crate) async fn stalled_after_response(response: &'static str) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind stalled HTTP stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept stalled request");
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write partial HTTP response");
            std::future::pending::<()>().await;
        });
        Self { url, task }
    }

    // Protocol-error tests need bytes that an HTTP framework would repair or reject.
    pub(crate) async fn raw(response: String) -> Self {
        Self::raw_with_request(response).await.0
    }

    pub(crate) async fn raw_with_request(
        response: String,
    ) -> (Self, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind raw HTTP stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let (sent, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept HTTP request");
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let read = socket.read(&mut buffer).await.expect("read HTTP request");
                assert!(read > 0, "request ended before its declared body");
                request.extend_from_slice(&buffer[..read]);
                let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") else {
                    continue;
                };
                let length = String::from_utf8_lossy(&request[..end])
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
                if request.len() >= end + 4 + length {
                    break;
                }
            }
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write raw HTTP response");
            let _ = sent.send(request);
        });
        (Self { url, task }, received)
    }

    pub(crate) async fn start(router: Router) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind external HTTP stub");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve external HTTP stub");
        });
        Self { url, task }
    }
}

impl Drop for HttpStub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn keyword_provider(
    width: usize,
) -> (
    Arc<dyn EmbeddingProvider>,
    HttpStub,
    Arc<Mutex<Vec<String>>>,
) {
    use yorishiro::services::embedding::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

    let inputs = Arc::new(Mutex::new(Vec::new()));
    let captured = inputs.clone();
    let server = HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let captured = captured.clone();
            async move {
                let data = body["input"]
                    .as_array()
                    .expect("embedding input")
                    .iter()
                    .enumerate()
                    .map(|(index, text)| {
                        let text = text.as_str().expect("text input");
                        captured.lock().unwrap().push(text.to_owned());
                        let axis = ["alpha", "beta", "gamma"]
                            .iter()
                            .position(|word| text.contains(word))
                            .unwrap_or(3);
                        let mut vector = vec![0.0_f32; width];
                        vector[axis] = 1.0;
                        serde_json::json!({"index": index, "embedding": vector})
                    })
                    .collect::<Vec<_>>();
                axum::Json(serde_json::json!({"data": data}))
            }
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "keyword-test-provider".into(),
        dimensions: width,
        send_dimensions_param: false,
    });
    (Arc::new(provider), server, inputs)
}

pub(crate) async fn unavailable_provider() -> (Arc<dyn EmbeddingProvider>, HttpStub) {
    use yorishiro::services::embedding::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

    let server = HttpStub::start(axum::Router::new().route(
        "/embeddings",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "private upstream diagnostic",
            )
        }),
    ))
    .await;
    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: server.url.clone(),
        api_key: "test-key".into(),
        model: "unavailable-test-model".into(),
        dimensions: 768,
        send_dimensions_param: false,
    });
    (Arc::new(provider), server)
}
