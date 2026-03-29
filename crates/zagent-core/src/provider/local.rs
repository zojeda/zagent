use super::Provider;
use super::{HttpClient, HttpMethod, HttpRequest, ProviderModel};
use crate::Result;
use async_trait::async_trait;
use serde::Deserialize;

/// Local OpenAI-compatible provider for servers like LM Studio or llama.cpp.
pub struct LocalProvider {
    api_key: Option<String>,
    base_url: String,
}

impl LocalProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            api_key: None,
            base_url: base_url.into(),
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Provider for LocalProvider {
    fn name(&self) -> &str {
        "local"
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> &str {
        self.api_key.as_deref().unwrap_or("")
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        self.api_key
            .as_ref()
            .map(|api_key| vec![("Authorization".to_string(), format!("Bearer {api_key}"))])
            .unwrap_or_default()
    }

    async fn list_models(&self, http_client: &dyn HttpClient) -> Result<Vec<ProviderModel>> {
        let request = HttpRequest {
            url: self.models_url(),
            method: HttpMethod::Get,
            headers: self.auth_headers(),
            body: None,
        };
        let response = http_client.send(request).await?;
        if response.status >= 400 {
            return Err(crate::Error::api(
                response.status,
                format!(
                    "Local model list request failed: {}",
                    response.body.chars().take(500).collect::<String>()
                ),
            ));
        }

        let parsed: OpenAiModelsResponse = serde_json::from_str(&response.body).map_err(|e| {
            crate::Error::provider(self.name(), format!("Invalid model list response: {e}"))
        })?;

        let mut models: Vec<ProviderModel> = parsed
            .data
            .into_iter()
            .map(|model| ProviderModel {
                id: model.id,
                name: model.name,
                description: model.description,
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::{ChatRequest, Message};
    use crate::provider::{HttpResponse, Provider};
    use async_trait::async_trait;
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct RecordingHttpClient {
        requests: Arc<Mutex<Vec<HttpRequest>>>,
        response: Arc<Mutex<Option<HttpResponse>>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl HttpClient for RecordingHttpClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
            self.requests.lock().expect("requests lock").push(request);
            self.response
                .lock()
                .expect("response lock")
                .clone()
                .ok_or_else(|| crate::Error::provider("local", "missing test response"))
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Wake, Waker};

        struct NoopWake;
        impl Wake for NoopWake {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(NoopWake));
        let mut future = pin!(future);
        let mut context = Context::from_waker(&waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future unexpectedly pending"),
        }
    }

    #[test]
    fn build_http_request_omits_authorization_without_api_key() {
        let provider = LocalProvider::new("http://127.0.0.1:1234/v1");
        let request = provider
            .build_http_request(&ChatRequest::new("local-model", vec![Message::user("hi")]))
            .expect("request");

        assert_eq!(request.url, "http://127.0.0.1:1234/v1/chat/completions");
        assert!(
            !request
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("Authorization"))
        );
    }

    #[test]
    fn build_http_request_includes_authorization_with_api_key() {
        let provider = LocalProvider::new("http://127.0.0.1:1234/v1").with_api_key("local-key");
        let request = provider
            .build_http_request(&ChatRequest::new("local-model", vec![Message::user("hi")]))
            .expect("request");

        assert!(
            request.headers.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("Authorization") && v == "Bearer local-key"
            })
        );
    }

    #[test]
    fn list_models_uses_openai_compatible_models_endpoint() {
        let provider = LocalProvider::new("http://127.0.0.1:1234/v1").with_api_key("local-key");
        let client = RecordingHttpClient::default();
        *client.response.lock().expect("response lock") = Some(HttpResponse {
            status: 200,
            body: serde_json::json!({
                "object": "list",
                "data": [
                    {"id": "qwen2.5-coder-7b-instruct", "name": "Qwen 2.5 Coder 7B"},
                    {"id": "llama-3.1-8b-instruct"}
                ]
            })
            .to_string(),
            headers: vec![],
        });

        let models = block_on(provider.list_models(&client)).expect("models");
        let requests = client.requests.lock().expect("requests lock");
        let request = requests.first().expect("request");

        assert_eq!(request.url, "http://127.0.0.1:1234/v1/models");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "llama-3.1-8b-instruct");
        assert_eq!(models[1].id, "qwen2.5-coder-7b-instruct");
        assert!(
            request.headers.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("Authorization") && v == "Bearer local-key"
            })
        );
    }
}
