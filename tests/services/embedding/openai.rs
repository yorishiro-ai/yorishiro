use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use yorishiro::YorishiroError;
use yorishiro::services::embedding::{
    EmbeddingProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
};

async fn provider_error(status: u16) -> YorishiroError {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("test listener must bind");
    let address = listener.local_addr().expect("test listener has an address");
    let body = "do-not-render-provider-value";
    let response = format!(
        "HTTP/1.1 {status} Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("test request must arrive");
        socket
            .write_all(response.as_bytes())
            .await
            .expect("test response must be written");
    });

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: format!("http://{address}"),
        api_key: "do-not-render-api-key".into(),
        model: "model".into(),
        dimensions: 1,
        send_dimensions_param: false,
    });

    provider
        .embed_batch(&["text"])
        .await
        .expect_err("the test provider must return an error")
}

async fn provider_response(status: u16, body: &str) -> Result<Vec<Vec<f32>>, YorishiroError> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("test listener must bind");
    let address = listener.local_addr().expect("test listener has an address");
    let response = format!(
        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("test request must arrive");
        socket
            .write_all(response.as_bytes())
            .await
            .expect("test response must be written");
    });

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: format!("http://{address}"),
        api_key: "do-not-render-api-key".into(),
        model: "model".into(),
        dimensions: 2,
        send_dimensions_param: true,
    });

    provider.embed_batch(&["first", "second"]).await
}

#[tokio::test]
async fn provider_returns_successful_batch_in_provider_order() {
    let vectors = provider_response(
        200,
        r#"{"data":[{"embedding":[1.0,2.0]},{"embedding":[3.0,4.0]}]}"#,
    )
    .await
    .expect("the test provider must return vectors");

    assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
}

#[tokio::test]
async fn malformed_provider_response_omits_body_and_api_key() {
    let error = provider_response(200, "do-not-render-provider-value")
        .await
        .expect_err("the test provider must return malformed JSON");
    let rendered = format!("{error:?}");

    assert!(!rendered.contains("do-not-render-provider-value"));
    assert!(!rendered.contains("do-not-render-api-key"));
}

#[tokio::test]
async fn provider_busy_error_omits_response_body() {
    let error = provider_error(503).await;
    let rendered = format!("{error:?}");

    assert!(!rendered.contains("do-not-render-provider-value"));
}

#[tokio::test]
async fn provider_internal_error_omits_response_body() {
    let error = provider_error(500).await;
    let rendered = format!("{error:?}");

    assert!(!rendered.contains("do-not-render-provider-value"));
}
