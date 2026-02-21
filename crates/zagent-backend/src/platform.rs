use async_trait::async_trait;
use reqwest::Client;

use zagent_core::Result;
use zagent_core::provider::{HttpClient, HttpMethod, HttpRequest, HttpResponse};

/// Native HTTP client backed by reqwest.
pub struct NativeHttpClient {
    client: Client,
}

impl Default for NativeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeHttpClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("failed to build reqwest client");

        Self { client }
    }
}

#[async_trait]
impl HttpClient for NativeHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        let mut builder = match request.method {
            HttpMethod::Get => self.client.get(&request.url),
            HttpMethod::Post => self.client.post(&request.url),
        };

        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }

        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }

        let response = builder
            .send()
            .await
            .map_err(|e| zagent_core::Error::provider("http", format!("request failed: {e}")))?;

        let status = response.status().as_u16();

        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let body = response.text().await.map_err(|e| {
            zagent_core::Error::provider("http", format!("failed to read body: {e}"))
        })?;

        Ok(HttpResponse {
            status,
            body,
            headers,
        })
    }
}
