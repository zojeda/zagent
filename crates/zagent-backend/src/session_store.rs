use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::warn;

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;

use crate::migrations::{MigrationStatus, Migrator};
use zagent_core::Result;
use zagent_core::provider::types::Message;
use zagent_core::session::{
    SessionEvent, SessionMeta, SessionState, SessionStore, ToolExecutionRecord,
};

/// SurrealDB-backed session store (remote server via ws/http endpoint).
#[derive(Clone)]
pub struct SurrealSessionStore {
    db: Surreal<Any>,
}

#[derive(Debug, Deserialize)]
struct LegacySessionRow {
    session_id: String,
    payload_json: String,
}

#[derive(Debug, Deserialize)]
struct SessionRow {
    session_id: String,
    name: String,
    provider: Option<String>,
    model: Option<String>,
    created_at: String,
    updated_at: String,
    message_count: u32,
    total_prompt_tokens: u64,
    total_completion_tokens: u64,
    system_prompt: Option<String>,
    working_dir: Option<String>,
    messages: Option<Vec<Message>>,
    tool_executions: Option<Vec<ToolExecutionRecord>>,
}

impl SurrealSessionStore {
    pub async fn new(endpoint: &str) -> Result<Self> {
        let username = std::env::var("SURREALDB_USER").unwrap_or_else(|_| "root".to_string());
        let password = std::env::var("SURREALDB_PASS").unwrap_or_else(|_| "root".to_string());
        let namespace = std::env::var("SURREALDB_NS").unwrap_or_else(|_| "zagent".to_string());
        let database =
            std::env::var("SURREALDB_DB").unwrap_or_else(|_| "session_storage".to_string());
        let auto_migrate_latest = env_bool("ZAGENT_AUTO_MIGRATE_LATEST", cfg!(debug_assertions));

        let db = connect(endpoint)
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to open SurrealDB: {e}")))?;

        db.signin(Root { username, password }).await.map_err(|e| {
            zagent_core::Error::session(format!("Failed to authenticate SurrealDB: {e}"))
        })?;

        db.use_ns(&namespace).use_db(&database).await.map_err(|e| {
            zagent_core::Error::session(format!("Failed to select namespace/database: {e}"))
        })?;

        let migrator = Migrator::new(&db);
        // Always initialize migration metadata; optionally apply latest.
        let _ = migrator.status().await?;
        if auto_migrate_latest {
            migrator.migrate_to_latest().await?;
        }
        let store = Self { db };
        store.migrate_legacy_sessions().await?;
        store.migrate_legacy_events().await?;

        Ok(store)
    }

    pub async fn migration_status(&self) -> Result<Vec<MigrationStatus>> {
        Migrator::new(&self.db).status().await
    }

    pub async fn current_migration_version(&self) -> Result<u32> {
        Migrator::new(&self.db).current_version().await
    }

    pub async fn migrate_to_latest(&self) -> Result<()> {
        Migrator::new(&self.db).migrate_to_latest().await
    }

    pub async fn migrate_to(&self, target_version: u32) -> Result<()> {
        Migrator::new(&self.db).migrate_to(target_version).await
    }

    pub async fn subscribe_session_events(
        &self,
        session_id: &str,
    ) -> Result<mpsc::UnboundedReceiver<SessionEvent>> {
        let mut response = self
            .db
            .query(
                "LIVE SELECT event_id, session_id, IF session_ref = NONE THEN NONE ELSE <string>session_ref END AS session_ref, sequence, parent_event_id, IF parent_event_ref = NONE THEN NONE ELSE <string>parent_event_ref END AS parent_event_ref, kind, phase, agent, handoff_depth, turn, provider, IF provider_ref = NONE THEN NONE ELSE <string>provider_ref END AS provider_ref, model, IF model_ref = NONE THEN NONE ELSE <string>model_ref END AS model_ref, tool_name, success, finish_reason, latency_ms, prompt_tokens, completion_tokens, total_tokens, cached_prompt_tokens, cost_usd, credits_remaining, arguments, result, message, payload, <string>created_at AS created_at FROM session_events WHERE session_ref = type::record('sessions', $session_id)",
            )
            .bind(("session_id", session_id.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to start live query: {e}")))?;

        let mut stream = response
            .stream::<surrealdb::Notification<serde_json::Value>>(0)
            .map_err(|e| zagent_core::Error::session(format!("Failed to open live stream: {e}")))?;

        let (tx, rx) = mpsc::unbounded_channel::<SessionEvent>();
        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                let Ok(notification) = item else {
                    if let Err(err) = item {
                        warn!(error = %err, "Failed to read live session event notification");
                    }
                    continue;
                };
                match serde_json::from_value::<SessionEvent>(notification.data) {
                    Ok(event) => {
                        let _ = tx.send(event);
                    }
                    Err(err) => {
                        warn!(error = %err, "Failed to decode live session event notification");
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn migrate_legacy_sessions(&self) -> Result<()> {
        let mut response = self
            .db
            .query("SELECT session_id, payload_json FROM sessions WHERE payload_json != NONE")
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to query legacy sessions: {e}"))
            })?;

        let rows: Vec<serde_json::Value> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode legacy session rows: {e}"))
        })?;
        let rows: Vec<LegacySessionRow> = decode_rows(rows, "legacy session rows")?;

        for row in rows {
            let mut session = decode_session_payload(&row.payload_json)?;
            session.meta.id = row.session_id;
            self.save_session(&session).await?;
        }

        Ok(())
    }

    async fn migrate_legacy_events(&self) -> Result<()> {
        self.db
            .query(
                "UPDATE session_events SET session_ref = type::record('sessions', session_id) WHERE session_ref = NONE AND session_id != NONE",
            )
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to migrate legacy event session refs: {e}"))
            })?;

        Ok(())
    }

    async fn upsert_provider_and_model(
        &self,
        provider_name: &str,
        model_name: &str,
    ) -> Result<(String, String)> {
        let provider_id = slug_key(provider_name);
        let model_id = format!("{}__{}", provider_id, slug_key(model_name));

        self.db
            .query(
                "UPSERT type::record('providers', $provider_id) MERGE { provider_id: $provider_id, name: $provider_name, updated_at: time::now(), created_at: IF created_at = NONE THEN time::now() ELSE created_at END }",
            )
            .bind(("provider_id", provider_id.clone()))
            .bind(("provider_name", provider_name.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to upsert provider: {e}")))?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to upsert provider (statement): {e}"))
            })?;

        self.db
            .query(
                "UPSERT type::record('models', $model_id) MERGE { model_id: $model_id, name: $model_name, provider_name: $provider_name, provider_ref: type::record('providers', $provider_id), updated_at: time::now(), created_at: IF created_at = NONE THEN time::now() ELSE created_at END }",
            )
            .bind(("model_id", model_id.clone()))
            .bind(("model_name", model_name.to_string()))
            .bind(("provider_name", provider_name.to_string()))
            .bind(("provider_id", provider_id.clone()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to upsert model: {e}")))?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to upsert model (statement): {e}"))
            })?;

        Ok((provider_id, model_id))
    }

    async fn infer_parent_event_id(&self, event: &SessionEvent) -> Result<Option<String>> {
        let mut query = None::<String>;

        if event.kind == "model_response_received" {
            query = Some(
                "SELECT event_id, sequence FROM session_events WHERE session_ref = type::record('sessions', $session_id) AND kind = 'model_request_started' AND agent = $agent AND handoff_depth = $handoff_depth AND turn = $turn ORDER BY sequence DESC LIMIT 1".to_string(),
            );
        } else if event.kind == "tool_call_finished" {
            query = Some(
                "SELECT event_id, sequence FROM session_events WHERE session_ref = type::record('sessions', $session_id) AND kind = 'tool_call_started' AND agent = $agent AND handoff_depth = $handoff_depth AND tool_name = $tool_name ORDER BY sequence DESC LIMIT 1".to_string(),
            );
        } else if let Some(seq) = event.sequence {
            if seq > 1 {
                query = Some(
                    "SELECT event_id, sequence FROM session_events WHERE session_ref = type::record('sessions', $session_id) AND sequence < $sequence ORDER BY sequence DESC LIMIT 1".to_string(),
                );
            }
        }

        let Some(query) = query else {
            return Ok(None);
        };

        let mut response = self
            .db
            .query(query)
            .bind(("session_id", event.session_id.clone()))
            .bind(("sequence", event.sequence.unwrap_or(0)))
            .bind(("agent", event.agent.clone()))
            .bind(("handoff_depth", event.handoff_depth as u64))
            .bind(("turn", event.turn.map(u64::from)))
            .bind(("tool_name", event.tool_name.clone()))
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to infer parent event: {e}"))
            })?;

        let rows: Vec<serde_json::Value> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode inferred parent event: {e}"))
        })?;

        for row in rows.into_iter().rev() {
            if let Some(id) = row.get("event_id").and_then(|v| v.as_str()) {
                return Ok(Some(id.to_string()));
            }
        }

        Ok(None)
    }
}

fn decode_session_payload(payload_json: &str) -> Result<SessionState> {
    serde_json::from_str(payload_json)
        .map_err(|e| zagent_core::Error::session(format!("Invalid session payload_json: {e}")))
}

fn slug_key(input: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

fn decode_rows<T: DeserializeOwned>(rows: Vec<serde_json::Value>, label: &str) -> Result<Vec<T>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(decode_value(row, label)?);
    }
    Ok(out)
}

fn decode_value<T: DeserializeOwned>(value: serde_json::Value, label: &str) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|e| zagent_core::Error::session(format!("Failed to decode {label}: {e}")))
}

async fn reconstruct_conversation_from_events(
    store: &SurrealSessionStore,
    session_id: &str,
) -> Result<Option<(Vec<Message>, Vec<ToolExecutionRecord>)>> {
    let events = store.list_events(session_id, None).await?;
    if events.is_empty() {
        return Ok(None);
    }

    let mut checkpoint_seq = 0_u64;
    let mut messages = Vec::<Message>::new();
    let mut tool_executions = Vec::<ToolExecutionRecord>::new();

    for event in &events {
        if event.kind != "conversation_checkpoint" {
            continue;
        }
        let seq = event.sequence.unwrap_or(0);
        if seq < checkpoint_seq {
            continue;
        }

        let checkpoint_messages = event
            .payload
            .get("messages")
            .cloned()
            .and_then(|v| serde_json::from_value::<Vec<Message>>(v).ok())
            .unwrap_or_default();
        let checkpoint_tool_executions = event
            .payload
            .get("tool_executions")
            .cloned()
            .and_then(|v| serde_json::from_value::<Vec<ToolExecutionRecord>>(v).ok())
            .unwrap_or_default();

        checkpoint_seq = seq;
        messages = checkpoint_messages;
        tool_executions = checkpoint_tool_executions;
    }

    let mut saw_canonical = checkpoint_seq > 0;
    for event in events {
        if event.sequence.unwrap_or(0) <= checkpoint_seq {
            continue;
        }
        match event.kind.as_str() {
            "user_message_added" | "assistant_message_added" | "tool_result_added" => {
                if let Some(message_value) = event.payload.get("message")
                    && let Ok(message) = serde_json::from_value::<Message>(message_value.clone())
                {
                    messages.push(message);
                    saw_canonical = true;
                }
                if event.kind == "tool_result_added"
                    && let Some(exec_value) = event.payload.get("tool_execution")
                    && let Ok(exec) =
                        serde_json::from_value::<ToolExecutionRecord>(exec_value.clone())
                {
                    tool_executions.push(exec);
                }
            }
            _ => {}
        }
    }

    if !saw_canonical {
        return Ok(None);
    }

    Ok(Some((messages, tool_executions)))
}

fn parse_datetime(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| {
            zagent_core::Error::session(format!("Failed to parse datetime '{value}': {e}"))
        })
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

#[async_trait]
impl SessionStore for SurrealSessionStore {
    async fn save_session(&self, session: &SessionState) -> Result<()> {
        let (provider_id, model_id) = self
            .upsert_provider_and_model(&session.meta.provider, &session.meta.model)
            .await?;

        self.db
            .query(
                "UPSERT type::record('sessions', $id) CONTENT { session_id: $session_id, name: $name, provider: $provider, provider_ref: type::record('providers', $provider_id), model: $model, model_ref: type::record('models', $model_id), created_at: <datetime>$created_at, updated_at: <datetime>$updated_at, message_count: $message_count, total_prompt_tokens: $total_prompt_tokens, total_completion_tokens: $total_completion_tokens, system_prompt: $system_prompt, working_dir: $working_dir, messages: $messages, tool_executions: $tool_executions }",
            )
            .bind(("id", session.meta.id.clone()))
            .bind(("session_id", session.meta.id.clone()))
            .bind(("name", session.meta.name.clone()))
            .bind(("provider", session.meta.provider.clone()))
            .bind(("provider_id", provider_id))
            .bind(("model", session.meta.model.clone()))
            .bind(("model_id", model_id))
            .bind(("created_at", session.meta.created_at.to_rfc3339()))
            .bind(("updated_at", session.meta.updated_at.to_rfc3339()))
            .bind(("message_count", session.meta.message_count as u64))
            .bind(("total_prompt_tokens", session.meta.total_prompt_tokens))
            .bind(("total_completion_tokens", session.meta.total_completion_tokens))
            .bind(("system_prompt", session.system_prompt.clone()))
            .bind(("working_dir", session.working_dir.clone()))
            .bind(("messages", serde_json::json!([])))
            .bind(("tool_executions", serde_json::json!([])))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to save session: {e}")))?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to save session (statement): {e}"))
            })?;

        Ok(())
    }

    async fn load_session(&self, id: &str) -> Result<SessionState> {
        let mut response = self
            .db
            .query("SELECT session_id, name, provider, model, <string>created_at AS created_at, <string>updated_at AS updated_at, message_count, total_prompt_tokens, total_completion_tokens, system_prompt, working_dir, messages, tool_executions FROM ONLY type::record('sessions', $id)")
            .bind(("id", id.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to load session: {e}")))?;

        let row: Option<serde_json::Value> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode session row: {e}"))
        })?;

        let row: SessionRow = row
            .map(|value| decode_value(value, "session row"))
            .transpose()?
            .ok_or_else(|| zagent_core::Error::session(format!("Session '{id}' not found")))?;

        let mut session = SessionState {
            meta: SessionMeta {
                id: row.session_id,
                name: row.name,
                model: row.model.unwrap_or_else(|| "unknown".to_string()),
                provider: row.provider.unwrap_or_else(|| "unknown".to_string()),
                created_at: parse_datetime(&row.created_at)?,
                updated_at: parse_datetime(&row.updated_at)?,
                message_count: row.message_count,
                total_prompt_tokens: row.total_prompt_tokens,
                total_completion_tokens: row.total_completion_tokens,
            },
            system_prompt: row.system_prompt.unwrap_or_default(),
            working_dir: row.working_dir.unwrap_or_default(),
            messages: row.messages.unwrap_or_default(),
            tool_executions: row.tool_executions.unwrap_or_default(),
        };

        if session.messages.is_empty() {
            if let Some((messages, tool_executions)) =
                reconstruct_conversation_from_events(self, &session.meta.id).await?
            {
                session.messages = messages;
                if session.tool_executions.is_empty() {
                    session.tool_executions = tool_executions;
                }
                session.meta.message_count = session.messages.len() as u32;
            }
        }

        Ok(session)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let mut response = self
            .db
            .query("SELECT session_id, name, provider, model, <string>created_at AS created_at, <string>updated_at AS updated_at, message_count, total_prompt_tokens, total_completion_tokens FROM sessions ORDER BY updated_at DESC")
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to list sessions: {e}")))?;

        let rows: Vec<serde_json::Value> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode session rows: {e}"))
        })?;
        let rows: Vec<SessionRow> = decode_rows(rows, "session rows")?;

        rows.into_iter()
            .map(|row| {
                Ok(SessionMeta {
                    id: row.session_id,
                    name: row.name,
                    model: row.model.unwrap_or_else(|| "unknown".to_string()),
                    provider: row.provider.unwrap_or_else(|| "unknown".to_string()),
                    created_at: parse_datetime(&row.created_at)?,
                    updated_at: parse_datetime(&row.updated_at)?,
                    message_count: row.message_count,
                    total_prompt_tokens: row.total_prompt_tokens,
                    total_completion_tokens: row.total_completion_tokens,
                })
            })
            .collect()
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        self.db
            .query("DELETE type::record('sessions', $id)")
            .bind(("id", id.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to delete session: {e}")))?;

        self.db
            .query("DELETE session_events WHERE session_id = $session_id")
            .bind(("session_id", id.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to delete events: {e}")))?;

        Ok(())
    }

    async fn find_session_by_name(&self, name: &str) -> Result<Option<SessionState>> {
        let mut response = self
            .db
            .query("SELECT VALUE session_id FROM sessions WHERE name = $name LIMIT 1")
            .bind(("name", name.to_string()))
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to find session by name: {e}"))
            })?;

        let mut ids: Vec<String> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode session id: {e}"))
        })?;

        match ids.pop() {
            Some(id) => self.load_session(&id).await.map(Some),
            None => Ok(None),
        }
    }

    async fn append_event(&self, event: &SessionEvent) -> Result<()> {
        let mut event_to_store = event.clone();
        event_to_store.session_ref = Some(format!("sessions:{}", event_to_store.session_id));

        if event_to_store.provider.is_none() {
            event_to_store.provider = event_to_store
                .payload
                .get("provider")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
        }
        if event_to_store.model.is_none() {
            event_to_store.model = event_to_store
                .payload
                .get("model")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
        }

        if event_to_store.sequence.is_none() {
            let query_result = self
                .db
                .query(
                    "SELECT VALUE sequence FROM session_events WHERE session_ref = type::record('sessions', $session_id) ORDER BY sequence DESC LIMIT 1",
                )
                .bind(("session_id", event.session_id.clone()))
                .await;

            let next_seq = match query_result {
                Ok(mut response) => {
                    let latest: Vec<Option<u64>> = response.take(0).map_err(|e| {
                        zagent_core::Error::session(format!("Failed to decode event sequence: {e}"))
                    })?;
                    latest
                        .first()
                        .and_then(|v| *v)
                        .unwrap_or(0)
                        .saturating_add(1)
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("does not exist") && msg.contains("session_events") {
                        1
                    } else {
                        return Err(zagent_core::Error::session(format!(
                            "Failed to query event sequence: {e}"
                        )));
                    }
                }
            };
            event_to_store.sequence = Some(next_seq);
        }

        if event_to_store.parent_event_id.is_none() {
            event_to_store.parent_event_id = self.infer_parent_event_id(&event_to_store).await?;
        }

        if event_to_store.parent_event_ref.is_none() {
            event_to_store.parent_event_ref = event_to_store
                .parent_event_id
                .as_ref()
                .map(|id| format!("session_events:{id}"));
        }

        if let (Some(provider), Some(model)) = (
            event_to_store.provider.as_deref(),
            event_to_store.model.as_deref(),
        ) {
            let (provider_id, model_id) = self.upsert_provider_and_model(provider, model).await?;
            event_to_store.provider_ref = Some(format!("providers:{provider_id}"));
            event_to_store.model_ref = Some(format!("models:{model_id}"));
        }

        self.db
            .query(
                "UPSERT type::record('session_events', $id) CONTENT { event_id: $event_id, session_id: $session_id, session_ref: type::record('sessions', $session_id), sequence: $sequence, parent_event_id: $parent_event_id, parent_event_ref: IF $parent_event_id = NONE THEN NONE ELSE type::record('session_events', $parent_event_id) END, kind: $kind, phase: $phase, agent: $agent, handoff_depth: $handoff_depth, turn: $turn, provider: $provider, provider_ref: IF $provider_ref_id = NONE THEN NONE ELSE type::record('providers', $provider_ref_id) END, model: $model, model_ref: IF $model_ref_id = NONE THEN NONE ELSE type::record('models', $model_ref_id) END, tool_name: $tool_name, success: $success, finish_reason: $finish_reason, latency_ms: $latency_ms, prompt_tokens: $prompt_tokens, completion_tokens: $completion_tokens, total_tokens: $total_tokens, cached_prompt_tokens: $cached_prompt_tokens, cost_usd: $cost_usd, credits_remaining: $credits_remaining, arguments: $arguments, result: $result, payload: $payload, created_at: <datetime>$created_at }",
            )
            .bind(("id", event_to_store.id.clone()))
            .bind(("event_id", event_to_store.id.clone()))
            .bind(("session_id", event_to_store.session_id.clone()))
            .bind(("sequence", event_to_store.sequence))
            .bind(("parent_event_id", event_to_store.parent_event_id.clone()))
            .bind(("kind", event_to_store.kind.clone()))
            .bind(("phase", event_to_store.phase.clone()))
            .bind(("agent", event_to_store.agent.clone()))
            .bind(("handoff_depth", event_to_store.handoff_depth as u64))
            .bind(("turn", event_to_store.turn.map(u64::from)))
            .bind(("provider", event_to_store.provider.clone()))
            .bind((
                "provider_ref_id",
                event_to_store
                    .provider_ref
                    .as_deref()
                    .and_then(|v| v.split(':').nth(1))
                    .map(ToString::to_string),
            ))
            .bind(("model", event_to_store.model.clone()))
            .bind((
                "model_ref_id",
                event_to_store
                    .model_ref
                    .as_deref()
                    .and_then(|v| v.split(':').nth(1))
                    .map(ToString::to_string),
            ))
            .bind(("tool_name", event_to_store.tool_name.clone()))
            .bind(("success", event_to_store.success))
            .bind(("finish_reason", event_to_store.finish_reason.clone()))
            .bind(("latency_ms", event_to_store.latency_ms))
            .bind(("prompt_tokens", event_to_store.prompt_tokens))
            .bind(("completion_tokens", event_to_store.completion_tokens))
            .bind(("total_tokens", event_to_store.total_tokens))
            .bind(("cached_prompt_tokens", event_to_store.cached_prompt_tokens))
            .bind(("cost_usd", event_to_store.cost_usd))
            .bind(("credits_remaining", event_to_store.credits_remaining))
            .bind(("arguments", event_to_store.arguments.clone()))
            .bind(("result", event_to_store.result.clone()))
            .bind(("payload", event_to_store.payload.clone()))
            .bind(("created_at", event_to_store.created_at.to_rfc3339()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to append event: {e}")))?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to append event (statement): {e}"))
            })?;

        Ok(())
    }

    async fn list_events(
        &self,
        session_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<Vec<SessionEvent>> {
        let mut response = self
            .db
            .query(
                "SELECT event_id, session_id, IF session_ref = NONE THEN NONE ELSE <string>session_ref END AS session_ref, sequence, parent_event_id, IF parent_event_ref = NONE THEN NONE ELSE <string>parent_event_ref END AS parent_event_ref, kind, phase, agent, handoff_depth, turn, provider, IF provider_ref = NONE THEN NONE ELSE <string>provider_ref END AS provider_ref, model, IF model_ref = NONE THEN NONE ELSE <string>model_ref END AS model_ref, tool_name, success, finish_reason, latency_ms, prompt_tokens, completion_tokens, total_tokens, cached_prompt_tokens, cost_usd, credits_remaining, arguments, result, payload, <string>created_at AS created_at FROM session_events WHERE session_ref = type::record('sessions', $session_id) OR session_id = $session_id ORDER BY sequence ASC, created_at ASC",
            )
            .bind(("session_id", session_id.to_string()))
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to list events: {e}")))?;

        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|e| zagent_core::Error::session(format!("Failed to decode events: {e}")))?;
        let mut events: Vec<SessionEvent> = decode_rows(rows, "session events")?;

        if let Some(seq) = after_sequence {
            events.retain(|event| event.sequence.unwrap_or(0) > seq);
        }

        Ok(events)
    }
}
