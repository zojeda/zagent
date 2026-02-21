pub mod openrouter;
pub mod types;

use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use types::{ChatRequest, ChatResponse};

/// HTTP request abstraction — platform-agnostic
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub url: String,
    pub method: HttpMethod,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// HTTP response abstraction
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Platform-agnostic HTTP client trait.
/// Implemented by native (reqwest) and WASI (wasi:http) backends.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse>;
}

/// Provider trait — encapsulates provider-specific API differences.
///
/// Each LLM provider (OpenRouter, OpenAI, Ollama, etc.) implements this trait
/// to handle authentication, URL routing, model naming, and request/response quirks.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider display name
    fn name(&self) -> &str;

    /// Base URL for the API (e.g., "https://openrouter.ai/api/v1")
    fn base_url(&self) -> &str;

    /// API key for this provider
    fn api_key(&self) -> &str;

    /// Build authentication headers
    fn auth_headers(&self) -> Vec<(String, String)> {
        vec![(
            "Authorization".to_string(),
            format!("Bearer {}", self.api_key()),
        )]
    }

    /// Extra provider-specific headers
    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![]
    }

    /// Map a generic model name to provider-specific model string
    fn map_model_name(&self, model: &str) -> String {
        model.to_string()
    }

    /// Whether this provider supports tool/function calling
    fn supports_tools(&self) -> bool {
        true
    }

    /// Whether this provider supports streaming
    fn supports_streaming(&self) -> bool {
        true
    }

    /// Apply provider-specific modifications to the request
    fn prepare_request(&self, mut request: ChatRequest) -> ChatRequest {
        request.model = self.map_model_name(&request.model);
        request
    }

    /// Parse the response body — override for providers with non-standard response formats
    fn parse_response(&self, body: &str) -> Result<ChatResponse> {
        serde_json::from_str(body).map_err(crate::Error::Json)
    }

    /// Build the full chat completions URL
    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url())
    }

    /// Fetch available models from the provider.
    async fn list_models(&self, _http_client: &dyn HttpClient) -> Result<Vec<ProviderModel>> {
        Err(crate::Error::provider(
            self.name(),
            "model listing is not supported",
        ))
    }

    /// Build the complete HTTP request for a chat completion call
    fn build_http_request(&self, chat_request: &ChatRequest) -> Result<HttpRequest> {
        let mut request = self.prepare_request(chat_request.clone());
        // Keep provider requests OpenAI-compatible; reasoning payload is for observability only.
        for message in &mut request.messages {
            message.reasoning_details = None;
        }
        let body = serde_json::to_string(&request)?;

        let mut headers = self.auth_headers();
        headers.extend(self.extra_headers());
        headers.push(("Content-Type".to_string(), "application/json".to_string()));

        Ok(HttpRequest {
            url: self.chat_completions_url(),
            method: HttpMethod::Post,
            headers,
            body: Some(body),
        })
    }
}

/// Registry of available providers
pub struct ProviderRegistry {
    providers: std::collections::HashMap<String, Box<dyn Provider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
        }
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|k| k.as_str()).collect()
    }
}
