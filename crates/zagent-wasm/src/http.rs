use async_trait::async_trait;
use gloo_net::http::Request;
use zagent_core::Result;
use zagent_core::provider::{HttpClient, HttpMethod, HttpRequest, HttpResponse};

#[derive(Default)]
pub struct BrowserHttpClient;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl HttpClient for BrowserHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse> {
        let mut builder = match request.method {
            HttpMethod::Get => Request::get(&request.url),
            HttpMethod::Post => Request::post(&request.url),
        };

        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }

        if let Some(body) = request.body {
            builder = builder.body(body).map_err(|e| {
                zagent_core::Error::provider("http", format!("request body failed: {e}"))
            })?;
        }

        let response = builder
            .send()
            .await
            .map_err(|e| zagent_core::Error::provider("http", format!("request failed: {e}")))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| {
            zagent_core::Error::provider("http", format!("failed to read body: {e}"))
        })?;

        Ok(HttpResponse {
            status,
            body,
            headers: Vec::new(),
        })
    }
}
