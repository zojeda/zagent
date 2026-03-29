use super::Provider;
use super::{HttpClient, HttpMethod, HttpRequest, ProviderModel};
use crate::Result;
use async_trait::async_trait;
use serde::Deserialize;

/// OpenRouter provider implementation.
///
/// OpenRouter routes requests to multiple LLM providers (Anthropic, OpenAI, Google, etc.)
/// using an OpenAI-compatible API at https://openrouter.ai/api/v1.
pub struct OpenRouterProvider {
    api_key: String,
    base_url: String,
    app_name: String,
    app_url: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            app_name: "zAgent".to_string(),
            app_url: "https://github.com/zagent".to_string(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = name.into();
        self
    }

    pub fn with_app_url(mut self, url: impl Into<String>) -> Self {
        self.app_url = url.into();
        self
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> &str {
        &self.api_key
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![
            ("HTTP-Referer".to_string(), self.app_url.clone()),
            ("X-Title".to_string(), self.app_name.clone()),
        ]
    }

    // OpenRouter uses the `provider/model` format natively (e.g., "minimax/minimax-m2.5")
    // No model name mapping needed.
    fn map_model_name(&self, model: &str) -> String {
        model.to_string()
    }

    async fn list_models(&self, http_client: &dyn HttpClient) -> Result<Vec<ProviderModel>> {
        let mut headers = self.auth_headers();
        headers.extend(self.extra_headers());
        let request = HttpRequest {
            url: format!("{}/models", self.base_url()),
            method: HttpMethod::Get,
            headers,
            body: None,
        };
        let response = http_client.send(request).await?;
        if response.status >= 400 {
            return Err(crate::Error::api(
                response.status,
                format!(
                    "OpenRouter model list request failed: {}",
                    response.body.chars().take(500).collect::<String>()
                ),
            ));
        }

        let parsed =
            serde_json::from_str::<OpenRouterModelsResponse>(&response.body).map_err(|e| {
                crate::Error::provider(self.name(), format!("Invalid model list response: {e}"))
            })?;

        let mut models: Vec<ProviderModel> = parsed
            .data
            .into_iter()
            .map(|m| ProviderModel {
                id: m.id,
                name: m.name,
                description: m.description,
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    #[serde(default)]
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}
