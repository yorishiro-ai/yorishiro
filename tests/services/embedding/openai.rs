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

mod unit {
    use async_trait::async_trait;
    use reqwest::{Request, StatusCode, header::HeaderMap};

    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use yorishiro::error::YorishiroError;
    use yorishiro::services::embedding::EmbeddingProvider;

    use yorishiro::services::embedding::openai::*;

    struct FakeHttp {
        outcome: Mutex<Option<Result<OpenAiHttpResponse, OpenAiHttpError>>>,
        request: Mutex<Option<Request>>,
    }

    #[async_trait]
    impl OpenAiCompatibleHttp for FakeHttp {
        async fn send(
            &self,
            _client: &reqwest::Client,
            request: Request,
        ) -> Result<OpenAiHttpResponse, OpenAiHttpError> {
            *self.request.lock().unwrap() = Some(request);
            self.outcome.lock().unwrap().take().unwrap()
        }
    }

    fn config() -> OpenAiCompatibleConfig {
        OpenAiCompatibleConfig {
            base_url: "http://embedding.test/".into(),
            api_key: "test-api-key".into(),
            model: "test-model".into(),
            dimensions: 2,
            send_dimensions_param: true,
        }
    }

    fn response(status: u16, body: &str) -> OpenAiHttpResponse {
        OpenAiHttpResponse {
            status: StatusCode::from_u16(status).unwrap(),
            headers: HeaderMap::new(),
            body: Box::pin(std::future::ready(Ok(body.as_bytes().to_vec()))),
        }
    }

    fn body_error(message: &str) -> OpenAiHttpResponse {
        OpenAiHttpResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Box::pin(std::future::ready(Err(OpenAiHttpError::Response(
                message.into(),
            )))),
        }
    }

    fn provider(
        outcome: Result<OpenAiHttpResponse, OpenAiHttpError>,
    ) -> (OpenAiCompatibleProvider, Arc<FakeHttp>) {
        let http = Arc::new(FakeHttp {
            outcome: Mutex::new(Some(outcome)),
            request: Mutex::new(None),
        });
        let provider =
            OpenAiCompatibleProvider::with_http(config(), reqwest::Client::new(), http.clone());
        (provider, http)
    }

    #[tokio::test]
    async fn injected_transport_preserves_batch_request_and_order() {
        let (provider, http) = provider(Ok(response(
            200,
            r#"{"data":[{"embedding":[1.0,2.0]},{"embedding":[3.0,4.0]}]}"#,
        )));

        let vectors = provider.embed_batch(&["first", "second"]).await.unwrap();

        assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        let request = http.request.lock().unwrap().take().unwrap();
        assert_eq!(request.url().as_str(), "http://embedding.test/embeddings");
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer test-api-key"
        );
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            br#"{"model":"test-model","input":["first","second"],"dimensions":2}"#
        );
    }

    #[tokio::test]
    async fn malformed_response_is_internal_and_redacts_body() {
        let secret = "provider-secret-body";
        let (provider, _) = provider(Ok(response(200, secret)));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(&error, YorishiroError::Internal(_)));
        assert!(!format!("{error:?}").contains(secret));
        assert!(!format!("{error:?}").contains("test-api-key"));
    }

    #[tokio::test]
    async fn response_validation_rejects_wrong_count_and_dimensions() {
        let (first_provider, _) =
            provider(Ok(response(200, r#"{"data":[{"embedding":[1.0,2.0]}]}"#)));
        let error = first_provider
            .embed_batch(&["first", "second"])
            .await
            .unwrap_err();
        assert!(error.to_string().contains("1 vectors for 2 inputs"));

        let (second_provider, _) = provider(Ok(response(200, r#"{"data":[{"embedding":[1.0]}]}"#)));
        let error = second_provider.embed_batch(&["text"]).await.unwrap_err();
        assert!(error.to_string().contains("length 1 but expected 2"));
    }

    #[tokio::test]
    async fn transport_failure_maps_to_provider_unreachable() {
        let (provider, _) = provider(Err(OpenAiHttpError::Request("connection refused".into())));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
        assert_eq!(error.code().unwrap().as_str(), "provider_unreachable");
        assert!(!format!("{error:?}").contains("test-api-key"));
    }

    #[tokio::test]
    async fn timeout_maps_to_provider_unreachable() {
        let timeout_message =
            "error sending request for url (https://example.invalid): operation timed out";
        let (provider, _) = provider(Err(OpenAiHttpError::Timeout(timeout_message.into())));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::ProviderUnreachable { .. }));
        assert!(error.to_string().contains(timeout_message));
        assert!(!error.to_string().contains("test-api-key"));
    }

    #[tokio::test]
    async fn response_body_read_failure_maps_to_internal_and_redacts_api_key() {
        let (provider, _) = provider(Ok(body_error(
            "provider body read failed after echoing test-api-key",
        )));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(error, YorishiroError::Internal(_)));
        assert!(!error.to_string().contains("test-api-key"));
    }

    #[tokio::test]
    async fn provider_status_mapping_omits_body() {
        let (provider, _) = provider(Ok(response(503, "provider-secret-body")));

        let error = provider.embed_batch(&["text"]).await.unwrap_err();

        assert!(matches!(
            &error,
            YorishiroError::ProviderBusy { retry_after, .. }
                if *retry_after == Duration::from_secs(5)
        ));
        assert!(!format!("{error:?}").contains("provider-secret-body"));
    }
}
