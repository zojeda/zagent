use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::types::Message;
use crate::time::utc_now;

/// Metadata about a session (for listing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub name: String,
    pub model: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: u32,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
}

/// Full session state (for save/load)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub meta: SessionMeta,
    pub system_prompt: String,
    pub working_dir: String,
    pub messages: Vec<Message>,
    pub tool_executions: Vec<ToolExecutionRecord>,
}

/// Record of a single tool execution (for observability)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: String,
    pub success: bool,
    pub latency_ms: u64,
    pub created_at: DateTime<Utc>,
}

/// Persisted runtime event for a session (model calls, tool calls, etc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    #[serde(rename = "event_id")]
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub session_ref: Option<String>,
    pub sequence: Option<u64>,
    #[serde(default)]
    pub parent_event_id: Option<String>,
    #[serde(default)]
    pub parent_event_ref: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub phase: Option<String>,
    pub agent: String,
    pub handoff_depth: u32,
    pub turn: Option<u32>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub provider_ref: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_ref: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub cached_prompt_tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub credits_remaining: Option<f64>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub result: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

impl SessionEvent {
    pub fn new(
        session_id: impl Into<String>,
        kind: impl Into<String>,
        agent: impl Into<String>,
        handoff_depth: u32,
        turn: Option<u32>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            session_ref: None,
            sequence: None,
            parent_event_id: None,
            parent_event_ref: None,
            kind: kind.into(),
            phase: None,
            agent: agent.into(),
            handoff_depth,
            turn,
            provider: None,
            provider_ref: None,
            model: None,
            model_ref: None,
            tool_name: None,
            tool_call_id: None,
            success: None,
            finish_reason: None,
            latency_ms: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            cached_prompt_tokens: None,
            cost_usd: None,
            credits_remaining: None,
            arguments: None,
            result: None,
            payload,
            created_at: utc_now(),
        }
    }
}

impl SessionState {
    pub fn new(
        name: impl Into<String>,
        model: impl Into<String>,
        provider: impl Into<String>,
        system_prompt: impl Into<String>,
        working_dir: impl Into<String>,
    ) -> Self {
        let now = utc_now();
        let id = Uuid::new_v4().to_string();
        Self {
            meta: SessionMeta {
                id,
                name: name.into(),
                model: model.into(),
                provider: provider.into(),
                created_at: now,
                updated_at: now,
                message_count: 0,
                total_prompt_tokens: 0,
                total_completion_tokens: 0,
            },
            system_prompt: system_prompt.into(),
            working_dir: working_dir.into(),
            messages: Vec::new(),
            tool_executions: Vec::new(),
        }
    }

    pub fn add_message(&mut self, message: Message) {
        self.meta.message_count += 1;
        self.meta.updated_at = utc_now();
        self.messages.push(message);
    }

    pub fn record_tool_execution(&mut self, record: ToolExecutionRecord) {
        self.tool_executions.push(record);
    }

    pub fn update_token_usage(&mut self, prompt_tokens: u64, completion_tokens: u64) {
        self.meta.total_prompt_tokens += prompt_tokens;
        self.meta.total_completion_tokens += completion_tokens;
        self.meta.updated_at = utc_now();
    }
}

/// Trait for session persistence — implemented differently on native (embedded DB) vs WASI (JSON files)
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn save_session(&self, session: &SessionState) -> crate::Result<()>;
    async fn load_session(&self, id: &str) -> crate::Result<SessionState>;
    async fn list_sessions(&self) -> crate::Result<Vec<SessionMeta>>;
    async fn delete_session(&self, id: &str) -> crate::Result<()>;
    async fn find_session_by_name(&self, name: &str) -> crate::Result<Option<SessionState>>;
    async fn append_event(&self, _event: &SessionEvent) -> crate::Result<()> {
        Ok(())
    }
    async fn list_events(
        &self,
        _session_id: &str,
        _after_sequence: Option<u64>,
    ) -> crate::Result<Vec<SessionEvent>> {
        Ok(Vec::new())
    }
}
