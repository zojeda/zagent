use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StreamChunk {
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub event: Option<UiEvent>,
    #[serde(default)]
    pub response: Option<BackendMessageResponse>,
    #[serde(default)]
    pub snapshot: Option<BackendSnapshot>,
    #[serde(default)]
    pub submitted: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UiEvent {
    pub kind: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BackendMessageResponse {
    #[serde(default)]
    pub output: String,
    pub state: BackendSnapshot,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BackendSnapshot {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub session: BackendSession,
    #[serde(default)]
    pub conversation: Vec<ConversationLine>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BackendSession {
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConversationLine {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
struct MessageRequest {
    input: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ApiErrorResponse {
    #[serde(default)]
    error: String,
}

#[derive(Debug, Clone)]
pub struct ProxyClient {
    pub base_url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelEventDetailsSnapshot {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub events: Vec<ModelEventDetail>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelEventDetail {
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub sequence: Option<u64>,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub turn: Option<u32>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub json_detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionUiEventsSnapshot {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub events: Vec<UiEvent>,
}

impl ProxyClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: normalize_base_url(&base_url.into()),
        }
    }

    pub fn events_stream_url(&self) -> String {
        format!("{}/api/events/stream", self.base_url)
    }

    pub async fn fetch_state(&self) -> Result<BackendSnapshot, String> {
        fetch_state(&self.base_url).await
    }

    pub async fn start_message(&self, input: &str) -> Result<(), String> {
        start_message(&self.base_url, input).await
    }

    pub async fn fetch_model_event_details(
        &self,
        session_id: Option<&str>,
        limit: Option<usize>,
        sequence: Option<u64>,
        event_id: Option<&str>,
    ) -> Result<ModelEventDetailsSnapshot, String> {
        fetch_model_event_details(&self.base_url, session_id, limit, sequence, event_id).await
    }

    pub async fn fetch_session_ui_events(
        &self,
        session_id: Option<&str>,
    ) -> Result<SessionUiEventsSnapshot, String> {
        fetch_session_ui_events(&self.base_url, session_id).await
    }
}

pub fn normalize_base_url(url: &str) -> String {
    url.trim_end_matches('/').trim().to_string()
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_state(base_url: &str) -> Result<BackendSnapshot, String> {
    let resp = gloo_net::http::Request::get(&format!("{}/api/state", normalize_base_url(base_url)))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<BackendSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_state(base_url: &str) -> Result<BackendSnapshot, String> {
    reqwest::Client::new()
        .get(format!("{}/api/state", normalize_base_url(base_url)))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<BackendSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(target_arch = "wasm32")]
pub async fn start_message(base_url: &str, input: &str) -> Result<(), String> {
    let body = MessageRequest {
        input: input.to_string(),
    };

    gloo_net::http::Request::post(&format!(
        "{}/api/message/start",
        normalize_base_url(base_url)
    ))
    .json(&body)
    .map_err(|e| format!("request build failed: {:?}", e))?
    .send()
    .await
    .map_err(|e| format!("request failed: {e}"))?;

    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_model_event_details(
    base_url: &str,
    session_id: Option<&str>,
    limit: Option<usize>,
    sequence: Option<u64>,
    event_id: Option<&str>,
) -> Result<ModelEventDetailsSnapshot, String> {
    let mut url = format!("{}/api/session/model-events", normalize_base_url(base_url));
    let mut query = Vec::new();
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        query.push(format!("session_id={}", urlencoding::encode(session_id)));
    }
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    if let Some(sequence) = sequence {
        query.push(format!("sequence={sequence}"));
    }
    if let Some(event_id) = event_id.filter(|s| !s.trim().is_empty()) {
        query.push(format!("event_id={}", urlencoding::encode(event_id)));
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    let resp = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if resp.status() >= 400 {
        let body = resp
            .json::<ApiErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| "model event request failed".to_string());
        return Err(body);
    }

    resp.json::<ModelEventDetailsSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_session_ui_events(
    base_url: &str,
    session_id: Option<&str>,
) -> Result<SessionUiEventsSnapshot, String> {
    let mut url = format!("{}/api/session/ui-events", normalize_base_url(base_url));
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        url.push_str(&format!("?session_id={}", urlencoding::encode(session_id)));
    }
    let resp = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if resp.status() >= 400 {
        let body = resp
            .json::<ApiErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| "session events request failed".to_string());
        return Err(body);
    }
    resp.json::<SessionUiEventsSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_model_event_details(
    base_url: &str,
    session_id: Option<&str>,
    limit: Option<usize>,
    sequence: Option<u64>,
    event_id: Option<&str>,
) -> Result<ModelEventDetailsSnapshot, String> {
    let mut url = format!("{}/api/session/model-events", normalize_base_url(base_url));
    let mut query = Vec::new();
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        query.push(format!("session_id={}", urlencoding::encode(session_id)));
    }
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    if let Some(sequence) = sequence {
        query.push(format!("sequence={sequence}"));
    }
    if let Some(event_id) = event_id.filter(|s| !s.trim().is_empty()) {
        query.push(format!("event_id={}", urlencoding::encode(event_id)));
    }
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let text = resp
            .json::<ApiErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| "model event request failed".to_string());
        return Err(text);
    }
    resp.json::<ModelEventDetailsSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_session_ui_events(
    base_url: &str,
    session_id: Option<&str>,
) -> Result<SessionUiEventsSnapshot, String> {
    let mut url = format!("{}/api/session/ui-events", normalize_base_url(base_url));
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        url.push_str(&format!("?session_id={}", urlencoding::encode(session_id)));
    }
    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let text = resp
            .json::<ApiErrorResponse>()
            .await
            .map(|e| e.error)
            .unwrap_or_else(|_| "session events request failed".to_string());
        return Err(text);
    }
    resp.json::<SessionUiEventsSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn start_message(base_url: &str, input: &str) -> Result<(), String> {
    reqwest::Client::new()
        .post(format!(
            "{}/api/message/start",
            normalize_base_url(base_url)
        ))
        .json(&MessageRequest {
            input: input.to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    Ok(())
}
