use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::{Instrument, Span, info_span};
use zagent_core::agent::{AgentConfig, AgentProgressEvent, run_agent_loop_with_progress};
use zagent_core::config::{ProviderConfig, load_config};
use zagent_core::provider::openrouter::OpenRouterProvider;
use zagent_core::provider::types::Role;
use zagent_core::provider::{Provider, ProviderModel};
use zagent_core::session::{
    SessionEvent, SessionMeta, SessionState, SessionStore, ToolExecutionRecord,
};

use crate::formatting::{format_reasoning_details, format_tool_call_text, infer_language};
use crate::mcp;
use crate::migrations::MigrationStatus;
use crate::runtime::{self, RuntimeBundle};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeTarget {
    Native,
    Wasi,
}

impl RuntimeTarget {
    pub fn parse(input: &str) -> Result<Self, zagent_core::Error> {
        match input {
            "native" => Ok(Self::Native),
            "wasi" => Ok(Self::Wasi),
            _ => Err(zagent_core::Error::config(
                "runtime must be one of: native, wasi",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wasi => "wasi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackendOptions {
    pub runtime: RuntimeTarget,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub working_dir: String,
    pub session_dir: Option<String>,
    pub resume_session: Option<String>,
    pub new_session: Option<String>,
    pub max_turns: u32,
}

impl Default for BackendOptions {
    fn default() -> Self {
        Self {
            runtime: RuntimeTarget::Native,
            model: None,
            system_prompt: None,
            working_dir: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            session_dir: None,
            resume_session: None,
            new_session: None,
            max_turns: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub name: String,
    pub model: String,
    pub message_count: u32,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendSnapshot {
    pub runtime: RuntimeTarget,
    pub provider: String,
    pub model: String,
    pub working_dir: String,
    pub session: SessionSnapshot,
    pub conversation: Vec<ConversationLine>,
    pub tool_results: Vec<ToolResultLine>,
    pub usage: UsageSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationLine {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolResultLine {
    pub tool_name: String,
    pub result: String,
    pub success: bool,
    pub latency_ms: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageSnapshot {
    pub total_tokens: u64,
    pub last_prompt_tokens: u64,
    pub cached_prompt_tokens: u64,
    pub tool_calls: u64,
    pub turns: u64,
    pub requests: u64,
    pub cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits_remaining: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackendMessageResponse {
    pub kind: String,
    pub output: String,
    pub turns: Option<u32>,
    pub tool_calls: Option<u32>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub events: Vec<UiEvent>,
    pub state: BackendSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiEvent {
    pub kind: String,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelEventDetail {
    pub event_id: String,
    pub sequence: Option<u64>,
    pub kind: String,
    pub agent: String,
    pub turn: Option<u32>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelEventDetailsSnapshot {
    pub session_id: String,
    pub events: Vec<ModelEventDetail>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelProviderCatalog {
    pub provider: String,
    pub model_count: usize,
    pub models: Vec<ProviderModel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCatalogSnapshot {
    pub current_provider: String,
    pub current_model: String,
    pub providers: Vec<ModelProviderCatalog>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpStatusSnapshot {
    pub runtime: String,
    pub servers: Vec<mcp::McpServerStatus>,
}

struct EngineState {
    config: AgentConfig,
    provider_name: String,
    session: SessionState,
    session_span: Span,
    runtime: RuntimeBundle,
    usage: UsageSnapshot,
}

struct EngineInner {
    providers: HashMap<String, Arc<dyn Provider>>,
    mcp_servers_config: std::collections::BTreeMap<String, zagent_core::config::McpServerConfig>,
    working_dir: String,
    session_dir: String,
    turn_lock: Mutex<()>,
    state: Mutex<EngineState>,
}

#[derive(Clone)]
pub struct BackendEngine {
    inner: Arc<EngineInner>,
}

impl BackendEngine {
    pub async fn new(options: BackendOptions) -> Result<Self, zagent_core::Error> {
        let app_config = load_config(&options.working_dir)?;
        let providers = build_providers(&app_config)?;
        if providers.is_empty() {
            return Err(zagent_core::Error::config(
                "No providers configured. Add providers to zagent-config.yaml or set OPENROUTER_API_KEY.",
            ));
        }
        let provider_name = select_initial_provider(&options, &app_config, &providers)?;

        let mut config = AgentConfig {
            model: options.model.unwrap_or_default(),
            max_turns: options.max_turns,
            ..AgentConfig::default()
        };
        if config.model.trim().is_empty() {
            config.model = resolve_default_model(&provider_name, &app_config)?;
        }
        if let Some(system_prompt) = options.system_prompt {
            config.system_prompt = system_prompt;
        }

        let session_dir = options.session_dir.unwrap_or_else(dirs_session_default);
        let mcp_manager = if options.runtime == RuntimeTarget::Native {
            Some(Arc::new(
                mcp::McpManager::start_servers(&app_config.mcp_servers, &options.working_dir).await,
            ))
        } else {
            None
        };
        let runtime = runtime::build_runtime(
            options.runtime,
            &session_dir,
            &options.working_dir,
            mcp_manager,
        )
        .await?;

        let mut session = resolve_session(
            runtime.session_store.as_ref(),
            options.resume_session.as_deref(),
            options.new_session.as_deref(),
            &provider_name,
            &config,
            &options.working_dir,
        )
        .await?;

        session.meta.model = config.model.clone();
        session.meta.provider = provider_name.clone();
        runtime.session_store.save_session(&session).await?;
        let session_span = build_session_span(&session);

        let inner = EngineInner {
            providers,
            mcp_servers_config: app_config.mcp_servers.clone(),
            working_dir: options.working_dir,
            session_dir,
            turn_lock: Mutex::new(()),
            state: Mutex::new(EngineState {
                config,
                provider_name,
                usage: usage_from_session(&session),
                session,
                session_span,
                runtime,
            }),
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    pub async fn snapshot(&self) -> BackendSnapshot {
        let state = self.inner.state.lock().await;
        snapshot_from_state(&state, &self.inner.working_dir)
    }

    pub async fn list_sessions(&self) -> Result<Vec<SessionMeta>, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let store = state.runtime.session_store.clone();
        drop(state);
        store.list_sessions().await
    }

    pub async fn session_events(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<SessionEvent>, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let resolved_session_id = session_id
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| state.session.meta.id.clone());
        let event_store = state.runtime.session_event_store.clone();
        drop(state);

        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Session events are not supported in this runtime")
        })?;
        store.list_events(&resolved_session_id, None).await
    }

    pub async fn model_event_details(
        &self,
        session_id: Option<&str>,
        limit: Option<usize>,
        sequence: Option<u64>,
        event_id: Option<&str>,
    ) -> Result<ModelEventDetailsSnapshot, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let resolved_session_id = session_id
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| state.session.meta.id.clone());
        let event_store = state.runtime.session_event_store.clone();
        drop(state);

        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Model event details are not supported in this runtime")
        })?;
        let mut details: Vec<ModelEventDetail> = store
            .list_events(&resolved_session_id, None)
            .await?
            .into_iter()
            .filter(|event| {
                event.kind == "model_request_started"
                    || event.kind == "model_response_received"
                    || event.kind == "tool_call_started"
                    || event.kind == "tool_call_finished"
            })
            .filter(|event| {
                sequence
                    .map(|target| event.sequence.unwrap_or(0) == target)
                    .unwrap_or(true)
            })
            .filter(|event| event_id.map(|target| event.id == target).unwrap_or(true))
            .map(|event| ModelEventDetail {
                event_id: event.id,
                sequence: event.sequence,
                kind: event.kind,
                agent: event.agent,
                turn: event.turn,
                provider: event.provider,
                model: event.model,
                created_at: event.created_at.to_rfc3339(),
                json_detail: event
                    .payload
                    .get("json_detail")
                    .cloned()
                    .filter(|v| !v.is_null()),
            })
            .collect();

        if let Some(limit) = limit.filter(|l| *l > 0) {
            if details.len() > limit {
                let drain = details.len() - limit;
                details.drain(0..drain);
            }
        }

        Ok(ModelEventDetailsSnapshot {
            session_id: resolved_session_id,
            events: details,
        })
    }

    pub async fn migration_status(&self) -> Result<Vec<MigrationStatus>, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let event_store = state.runtime.session_event_store.clone();
        drop(state);
        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Migrations are not supported in this runtime")
        })?;
        store.migration_status().await
    }

    pub async fn migrate_to_latest(&self) -> Result<u32, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let event_store = state.runtime.session_event_store.clone();
        drop(state);
        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Migrations are not supported in this runtime")
        })?;
        store.migrate_to_latest().await?;
        store.current_migration_version().await
    }

    pub async fn migrate_to(&self, target_version: u32) -> Result<u32, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let event_store = state.runtime.session_event_store.clone();
        drop(state);
        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Migrations are not supported in this runtime")
        })?;
        store.migrate_to(target_version).await?;
        store.current_migration_version().await
    }

    pub async fn subscribe_session_events(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<SessionEvent>, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let session_id = state.session.meta.id.clone();
        let event_store = state.runtime.session_event_store.clone();
        drop(state);

        let store = event_store.ok_or_else(|| {
            zagent_core::Error::session("Live session events are not supported in this runtime")
        })?;
        store.subscribe_session_events(&session_id).await
    }

    pub async fn list_models_catalog(&self) -> Result<ModelCatalogSnapshot, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let http_client = state.runtime.http_client.clone();
        let current_provider = state.provider_name.clone();
        let current_model = state.config.model.clone();
        drop(state);

        let mut providers = Vec::new();
        for (name, provider) in &self.inner.providers {
            let models = provider.list_models(http_client.as_ref()).await?;
            providers.push(ModelProviderCatalog {
                provider: name.clone(),
                model_count: models.len(),
                models,
            });
        }
        providers.sort_by(|a, b| a.provider.cmp(&b.provider));

        Ok(ModelCatalogSnapshot {
            current_provider,
            current_model,
            providers,
        })
    }

    pub async fn mcp_status_snapshot(&self) -> Result<McpStatusSnapshot, zagent_core::Error> {
        let state = self.inner.state.lock().await;
        let runtime = state.runtime.target.as_str().to_string();
        let manager = state.runtime.mcp_manager.clone();
        drop(state);
        let servers = if let Some(manager) = manager {
            manager.snapshot().await
        } else {
            self.inner
                .mcp_servers_config
                .iter()
                .map(|(name, cfg)| mcp::McpServerStatus {
                    name: name.clone(),
                    enabled: cfg.is_enabled(),
                    state: if cfg.is_enabled() {
                        "inactive".to_string()
                    } else {
                        "disabled".to_string()
                    },
                    tool_count: None,
                    tool_names: None,
                    error: None,
                })
                .collect()
        };
        Ok(McpStatusSnapshot { runtime, servers })
    }

    pub async fn set_model(
        &self,
        provider: Option<&str>,
        model: &str,
    ) -> Result<BackendMessageResponse, zagent_core::Error> {
        let _turn_guard = self.inner.turn_lock.lock().await;
        let mut state = self.inner.state.lock().await;
        let next_provider = provider
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(&state.provider_name)
            .to_string();
        if !self.inner.providers.contains_key(next_provider.as_str()) {
            return Err(zagent_core::Error::config(format!(
                "Provider '{next_provider}' is not configured"
            )));
        }
        state.provider_name = next_provider.clone();
        state.config.model = model.to_string();
        state.session.meta.provider = next_provider.clone();
        state.session.meta.model = model.to_string();
        state
            .runtime
            .session_store
            .save_session(&state.session)
            .await?;

        Ok(BackendMessageResponse {
            kind: "command".to_string(),
            output: format!("Model set to {} ({})", model, next_provider),
            turns: None,
            tool_calls: None,
            prompt_tokens: None,
            completion_tokens: None,
            events: vec![UiEvent {
                kind: "command".to_string(),
                title: "/model".to_string(),
                detail: "Model changed".to_string(),
                payload: None,
            }],
            state: snapshot_from_state(&state, &self.inner.working_dir),
        })
    }

    pub async fn send_input(
        &self,
        input: &str,
    ) -> Result<BackendMessageResponse, zagent_core::Error> {
        self.send_input_with_progress(input, |_event| {}).await
    }

    pub async fn send_input_with_progress<F>(
        &self,
        input: &str,
        mut on_progress: F,
    ) -> Result<BackendMessageResponse, zagent_core::Error>
    where
        F: FnMut(UiEvent) + Send,
    {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(BackendMessageResponse {
                kind: "noop".to_string(),
                output: "".to_string(),
                turns: None,
                tool_calls: None,
                prompt_tokens: None,
                completion_tokens: None,
                events: vec![UiEvent {
                    kind: "status".to_string(),
                    title: "No input".to_string(),
                    detail: "Input was empty; nothing sent to the agent.".to_string(),
                    payload: None,
                }],
                state: self.snapshot().await,
            });
        }

        let _turn_guard = self.inner.turn_lock.lock().await;

        if let Some(rest) = trimmed.strip_prefix('/') {
            return self.handle_command(rest).await;
        }

        let (
            mut config,
            manager,
            http_client,
            tools,
            store,
            session_span,
            provider_name,
            mut session,
            mut usage,
            start_tool_execs,
        ) = {
            let state = self.inner.state.lock().await;
            (
                state.config.clone(),
                state.runtime.mcp_manager.clone(),
                state.runtime.http_client.clone(),
                state.runtime.tools.clone(),
                state.runtime.session_store.clone(),
                state.session_span.clone(),
                state.provider_name.clone(),
                state.session.clone(),
                state.usage.clone(),
                state.session.tool_executions.len(),
            )
        };
        if let Some(manager) = manager.as_ref() {
            let mut names = Vec::new();
            for server in manager.snapshot().await {
                if server.state != "connected" {
                    continue;
                }
                if let Some(tool_names) = server.tool_names {
                    names.extend(tool_names);
                }
            }
            names.sort();
            names.dedup();
            config.visible_mcp_tools = names;
        } else {
            config.visible_mcp_tools.clear();
        }

        let provider = self
            .inner
            .providers
            .get(&provider_name)
            .ok_or_else(|| {
                zagent_core::Error::config(format!("Provider '{provider_name}' is not configured"))
            })?
            .clone();
        let mut streamed_events: Vec<UiEvent> = Vec::new();
        let mut stream_progress = |event: AgentProgressEvent| {
            if let Some(ui_event) = progress_event_to_ui_event(event) {
                streamed_events.push(ui_event.clone());
                on_progress(ui_event);
            }
        };

        let result = run_agent_loop_with_progress(
            http_client.as_ref(),
            provider.as_ref(),
            tools.as_ref(),
            &mut session,
            Some(store.as_ref()),
            &config,
            trimmed,
            Some(&mut stream_progress),
        )
        .instrument(session_span)
        .await?;

        usage.turns += result.turns as u64;
        usage.requests += 1;
        usage.tool_calls += result.tool_calls_count as u64;
        usage.last_prompt_tokens = result.last_prompt_tokens;
        usage.cached_prompt_tokens += result.total_cached_prompt_tokens;
        usage.cost_usd += result.total_cost_usd;
        if let Some(credits) = result.credits_remaining {
            usage.credits_remaining = Some(credits);
        }

        let mut events = vec![UiEvent {
            kind: "status".to_string(),
            title: "Agent response".to_string(),
            detail: format!(
                "turns={} tools={} tokens={}↑ {}↓",
                result.turns,
                result.tool_calls_count,
                result.total_prompt_tokens,
                result.total_completion_tokens
            ),
            payload: None,
        }];
        events.extend(streamed_events);
        for exec in session.tool_executions.iter().skip(start_tool_execs) {
            if let Some(change) = tool_change_event(exec) {
                events.push(change);
            }
        }

        let state_snapshot = {
            let mut state = self.inner.state.lock().await;
            state.session = session;
            state.usage = usage;
            snapshot_from_state(&state, &self.inner.working_dir)
        };

        Ok(BackendMessageResponse {
            kind: "assistant".to_string(),
            output: result.response,
            turns: Some(result.turns),
            tool_calls: Some(result.tool_calls_count),
            prompt_tokens: Some(result.total_prompt_tokens),
            completion_tokens: Some(result.total_completion_tokens),
            events,
            state: state_snapshot,
        })
    }

    async fn handle_command(
        &self,
        command_line: &str,
    ) -> Result<BackendMessageResponse, zagent_core::Error> {
        let mut parts = command_line.split_whitespace();
        let command = parts.next().unwrap_or_default();

        let output = match command {
            "help" | "h" => {
                "Commands:\n  /help\n  /model [model]\n  /model <provider> <model>\n  /mcp\n  /runtime [native|wasi]\n  /session list\n  /session new [name]\n  /session continue <name-or-id>".to_string()
            }
            "runtime" => {
                if let Some(runtime_name) = parts.next() {
                    let target = RuntimeTarget::parse(runtime_name)?;
                    let mut state = self.inner.state.lock().await;
                    if state.runtime.target == target {
                        format!("Runtime already set to {}", target.as_str())
                    } else {
                        let next_mcp = if target == RuntimeTarget::Native {
                            Some(Arc::new(
                                mcp::McpManager::start_servers(
                                    &self.inner.mcp_servers_config,
                                    &self.inner.working_dir,
                                )
                                .await,
                            ))
                        } else {
                            None
                        };
                        let next_runtime = runtime::build_runtime(
                            target,
                            &self.inner.session_dir,
                            &self.inner.working_dir,
                            next_mcp,
                        )
                        .await?;
                        let session_name = state.session.meta.name.clone();
                        let next_session = if let Some(existing) = next_runtime
                            .session_store
                            .find_session_by_name(&session_name)
                            .await?
                        {
                            existing
                        } else {
                            create_new_session(
                                Some(&session_name),
                                &state.provider_name,
                                &state.config,
                                &self.inner.working_dir,
                                next_runtime.session_store.as_ref(),
                            )
                            .await?
                        };

                        state.runtime = next_runtime;
                        state.usage = usage_from_session(&next_session);
                        state.session = next_session;
                        state.session_span = build_session_span(&state.session);
                        state.session.meta.provider = state.provider_name.clone();
                        state.session.meta.model = state.config.model.clone();
                        state
                            .runtime
                            .session_store
                            .save_session(&state.session)
                            .await?;

                        format!(
                            "Switched runtime to {}. Active session '{}' ({})",
                            target.as_str(),
                            state.session.meta.name,
                            state.session.meta.id
                        )
                    }
                } else {
                    let state = self.inner.state.lock().await;
                    format!("Current runtime: {}", state.runtime.target.as_str())
                }
            }
            "mcp" => {
                let snapshot = self.mcp_status_snapshot().await?;
                if snapshot.servers.is_empty() {
                    "No MCP servers configured.".to_string()
                } else {
                    let mut out = format!("MCP runtime={}\n", snapshot.runtime);
                    for s in snapshot.servers {
                        out.push_str(&format!(
                            "- {} enabled={} state={}",
                            s.name, s.enabled, s.state
                        ));
                        if let Some(count) = s.tool_count {
                            out.push_str(&format!(" tools={count}"));
                        }
                        if let Some(err) = s.error {
                            out.push_str(&format!(" error={err}"));
                        }
                        out.push('\n');
                    }
                    out.trim_end().to_string()
                }
            }
            "model" => {
                if let Some(model) = parts.next() {
                    if let Some(provider) = parts.next() {
                        return self.set_model(Some(model), provider).await;
                    }
                    return self.set_model(None, model).await;
                } else {
                    let state = self.inner.state.lock().await;
                    format!(
                        "Current model: {} ({})",
                        state.config.model, state.provider_name
                    )
                }
            }
            "session" => {
                let sub = parts.next().unwrap_or_default();
                match sub {
                    "list" => {
                        let state = self.inner.state.lock().await;
                        let sessions = state.runtime.session_store.list_sessions().await?;
                        if sessions.is_empty() {
                            "No sessions found.".to_string()
                        } else {
                            let mut out = String::from("Sessions:\n");
                            for s in sessions {
                                out.push_str(&format!(
                                    "- {} ({}) model={} msgs={}\n",
                                    s.name, s.id, s.model, s.message_count
                                ));
                            }
                            out.trim_end().to_string()
                        }
                    }
                    "new" => {
                        let name = parts.next();
                        let mut state = self.inner.state.lock().await;
                        let new_session = create_new_session(
                            name,
                            &state.provider_name,
                            &state.config,
                            &self.inner.working_dir,
                            state.runtime.session_store.as_ref(),
                        )
                        .await?;
                        state.usage = usage_from_session(&new_session);
                        state.session = new_session;
                        state.session_span = build_session_span(&state.session);
                        format!(
                            "Created new session '{}' ({})",
                            state.session.meta.name, state.session.meta.id
                        )
                    }
                    "continue" => {
                        let target = parts.next().ok_or_else(|| {
                            zagent_core::Error::config("usage: /session continue <name-or-id>")
                        })?;
                        let mut state = self.inner.state.lock().await;
                        let resumed =
                            resolve_existing_session(state.runtime.session_store.as_ref(), target)
                                .await?;
                        state.provider_name = resumed.meta.provider.clone();
                        state.config.model = resumed.meta.model.clone();
                        state.usage = usage_from_session(&resumed);
                        state.session = resumed;
                        state.session_span = build_session_span(&state.session);
                        format!(
                            "Resumed session '{}' ({})",
                            state.session.meta.name, state.session.meta.id
                        )
                    }
                    _ => {
                        "usage:\n  /session list\n  /session new [name]\n  /session continue <name-or-id>".to_string()
                    }
                }
            }
            _ => format!("Unknown command: /{}", command),
        };

        Ok(BackendMessageResponse {
            kind: "command".to_string(),
            output,
            turns: None,
            tool_calls: None,
            prompt_tokens: None,
            completion_tokens: None,
            events: vec![UiEvent {
                kind: "command".to_string(),
                title: format!("/{}", command),
                detail: "Command processed".to_string(),
                payload: None,
            }],
            state: self.snapshot().await,
        })
    }
}

fn build_session_span(session: &SessionState) -> Span {
    info_span!(
        "agent_session",
        session_id = %session.meta.id,
        session_name = %session.meta.name
    )
}

fn progress_event_to_ui_event(event: AgentProgressEvent) -> Option<UiEvent> {
    match event {
        AgentProgressEvent::ModelRequestStarted {
            agent,
            handoff_depth,
            turn,
            provider,
            model,
            message_count,
            tool_count,
            ..
        } => Some(UiEvent {
            kind: "model".to_string(),
            title: "model request".to_string(),
            detail: format!(
                "turn={} provider={} model={} msgs={} tools={}",
                turn, provider, model, message_count, tool_count
            ),
            payload: Some(serde_json::json!({
                "phase": "request_started",
                "agent": agent,
                "handoff_depth": handoff_depth,
                "turn": turn,
                "provider": provider,
                "model": model,
                "message_count": message_count,
                "tool_count": tool_count
            })),
        }),
        AgentProgressEvent::ModelResponseReceived {
            agent,
            handoff_depth,
            turn,
            finish_reason,
            latency_ms,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_prompt_tokens,
            cost_usd,
            credits_remaining,
            ..
        } => Some(UiEvent {
            kind: "model".to_string(),
            title: "model response".to_string(),
            detail: format!(
                "turn={} latency={}ms finish={} tokens={}↑ {}↓ total={}",
                turn,
                latency_ms,
                finish_reason.as_deref().unwrap_or("unknown"),
                prompt_tokens.unwrap_or(0),
                completion_tokens.unwrap_or(0),
                total_tokens.unwrap_or(0)
            ),
            payload: Some(serde_json::json!({
                "phase": "response_received",
                "agent": agent,
                "handoff_depth": handoff_depth,
                "turn": turn,
                "finish_reason": finish_reason,
                "latency_ms": latency_ms,
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": total_tokens,
                "cached_prompt_tokens": cached_prompt_tokens,
                "cost_usd": cost_usd,
                "credits_remaining": credits_remaining
            })),
        }),
        AgentProgressEvent::ToolCallStarted {
            agent,
            handoff_depth,
            tool_name,
            arguments,
        } => Some(UiEvent {
            kind: "tool".to_string(),
            title: format!("tool: {tool_name}"),
            detail: "started".to_string(),
            payload: Some(serde_json::json!({
                "phase": "start",
                "agent": agent,
                "handoff_depth": handoff_depth,
                "tool_name": tool_name,
                "arguments": arguments
            })),
        }),
        AgentProgressEvent::ToolCallFinished {
            agent,
            handoff_depth,
            tool_name,
            success,
            latency_ms,
            result,
        } => Some(UiEvent {
            kind: "tool".to_string(),
            title: format!("tool: {tool_name}"),
            detail: if success {
                format!("ok in {latency_ms}ms")
            } else {
                format!("failed in {latency_ms}ms")
            },
            payload: Some(serde_json::json!({
                "phase": "finish",
                "agent": agent,
                "handoff_depth": handoff_depth,
                "tool_name": tool_name,
                "success": success,
                "latency_ms": latency_ms,
                "result": result
            })),
        }),
    }
}

fn tool_change_event(exec: &ToolExecutionRecord) -> Option<UiEvent> {
    match exec.tool_name.as_str() {
        "file_edit" => {
            let path = exec
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown path)");
            const MAX_CHARS: usize = 20_000;
            let (diff, truncated) = exec
                .arguments
                .get("diff")
                .and_then(|v| v.as_str())
                .map(|c| {
                    let total = c.chars().count();
                    if total > MAX_CHARS {
                        (c.chars().take(MAX_CHARS).collect::<String>(), true)
                    } else {
                        (c.to_string(), false)
                    }
                })
                .unwrap_or_default();
            Some(UiEvent {
                kind: "change".to_string(),
                title: "file_edit".to_string(),
                detail: path.to_string(),
                payload: Some(serde_json::json!({
                    "path": path,
                    "diff": diff,
                    "truncated": truncated
                })),
            })
        }
        "file_write" => {
            let path = exec
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown path)");
            const MAX_CHARS: usize = 20_000;
            let (content, truncated) = exec
                .arguments
                .get("content")
                .and_then(|v| v.as_str())
                .map(|c| {
                    let total = c.chars().count();
                    if total > MAX_CHARS {
                        (c.chars().take(MAX_CHARS).collect::<String>(), true)
                    } else {
                        (c.to_string(), false)
                    }
                })
                .unwrap_or_default();
            Some(UiEvent {
                kind: "change".to_string(),
                title: "file_write".to_string(),
                detail: path.to_string(),
                payload: Some(serde_json::json!({
                    "path": path,
                    "content": content,
                    "truncated": truncated
                })),
            })
        }
        "shell_exec" => {
            let command = exec
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown command)");
            Some(UiEvent {
                kind: "change".to_string(),
                title: "shell_exec".to_string(),
                detail: command.to_string(),
                payload: None,
            })
        }
        "file_read" | "list_dir" => {
            let path = exec
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown path)");
            Some(UiEvent {
                kind: "change".to_string(),
                title: exec.tool_name.clone(),
                detail: path.to_string(),
                payload: None,
            })
        }
        _ => None,
    }
}

fn snapshot_from_state(state: &EngineState, working_dir: &str) -> BackendSnapshot {
    let session_total_tokens =
        state.session.meta.total_prompt_tokens + state.session.meta.total_completion_tokens;

    BackendSnapshot {
        runtime: state.runtime.target,
        provider: state.provider_name.clone(),
        model: state.config.model.clone(),
        working_dir: working_dir.to_string(),
        session: SessionSnapshot {
            id: state.session.meta.id.clone(),
            name: state.session.meta.name.clone(),
            model: state.session.meta.model.clone(),
            message_count: state.session.meta.message_count,
            total_prompt_tokens: state.session.meta.total_prompt_tokens,
            total_completion_tokens: state.session.meta.total_completion_tokens,
        },
        conversation: session_conversation_lines(&state.session),
        tool_results: state
            .session
            .tool_executions
            .iter()
            .map(|exec| ToolResultLine {
                tool_name: exec.tool_name.clone(),
                result: exec.result.clone(),
                success: exec.success,
                latency_ms: exec.latency_ms,
                created_at: exec.created_at.to_rfc3339(),
            })
            .collect(),
        usage: UsageSnapshot {
            total_tokens: session_total_tokens,
            ..state.usage.clone()
        },
    }
}

fn session_conversation_lines(session: &SessionState) -> Vec<ConversationLine> {
    let mut out = Vec::new();
    for msg in &session.messages {
        if let Some(tool_calls) = &msg.tool_calls {
            for call in tool_calls {
                if call.function.name == "file_edit"
                    && let Ok(args) =
                        serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                    && let Some(path) = args.get("path").and_then(|v| v.as_str())
                    && let Some(diff) = args.get("diff").and_then(|v| v.as_str())
                {
                    out.push(ConversationLine {
                        role: "file_diff".to_string(),
                        text: format!("path:{path}\nlang:{}\n{diff}", infer_language(path)),
                    });
                    continue;
                }
                out.push(ConversationLine {
                    role: "tool_call".to_string(),
                    text: format_tool_call_text(&call.function.name, &call.function.arguments),
                });
            }
        }

        let content_text = msg
            .content
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let reasoning_text = msg
            .reasoning_details
            .as_ref()
            .map(format_reasoning_details)
            .filter(|s| !s.is_empty());

        match msg.role {
            Role::System => {
                if let Some(text) = content_text {
                    out.push(ConversationLine {
                        role: "system".to_string(),
                        text,
                    });
                }
            }
            Role::User => {
                if let Some(text) = content_text {
                    out.push(ConversationLine {
                        role: "you".to_string(),
                        text,
                    });
                }
            }
            Role::Assistant => {
                if let Some(text) = reasoning_text {
                    out.push(ConversationLine {
                        role: "reasoning".to_string(),
                        text,
                    });
                }
                if let Some(text) = content_text {
                    out.push(ConversationLine {
                        role: "assistant".to_string(),
                        text,
                    });
                }
            }
            Role::Tool => {}
        }
    }
    out
}

fn usage_from_session(session: &SessionState) -> UsageSnapshot {
    UsageSnapshot {
        tool_calls: session.tool_executions.len() as u64,
        ..UsageSnapshot::default()
    }
}

async fn resolve_session(
    store: &dyn SessionStore,
    resume: Option<&str>,
    new_name: Option<&str>,
    provider_name: &str,
    config: &AgentConfig,
    working_dir: &str,
) -> Result<SessionState, zagent_core::Error> {
    if let Some(name_or_id) = resume {
        return resolve_existing_session(store, name_or_id).await;
    }

    create_new_session(new_name, provider_name, config, working_dir, store).await
}

async fn create_new_session(
    name: Option<&str>,
    provider_name: &str,
    config: &AgentConfig,
    working_dir: &str,
    store: &dyn SessionStore,
) -> Result<SessionState, zagent_core::Error> {
    let name = name.map(String::from).unwrap_or_else(|| {
        chrono::Utc::now()
            .format("session-%Y%m%d-%H%M%S")
            .to_string()
    });

    let session = SessionState::new(
        &name,
        &config.model,
        provider_name,
        &config.system_prompt,
        working_dir,
    );

    store.save_session(&session).await?;
    Ok(session)
}

async fn resolve_existing_session(
    store: &dyn SessionStore,
    name_or_id: &str,
) -> Result<SessionState, zagent_core::Error> {
    if let Some(session) = store.find_session_by_name(name_or_id).await? {
        return Ok(session);
    }

    store.load_session(name_or_id).await
}

fn build_providers(
    app_config: &zagent_core::config::ZagentConfig,
) -> Result<HashMap<String, Arc<dyn Provider>>, zagent_core::Error> {
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();

    for (provider_name, provider_config) in &app_config.providers {
        if !provider_config.is_enabled() {
            continue;
        }
        match provider_name.as_str() {
            "openrouter" => {
                let api_key = resolve_provider_api_key(provider_name, provider_config)?;
                let mut provider = OpenRouterProvider::new(api_key);
                if let Some(base_url) = resolve_provider_base_url(provider_name, provider_config) {
                    provider = provider.with_base_url(base_url);
                }
                if let Some(app_name) = resolve_provider_app_name(provider_name, provider_config) {
                    provider = provider.with_app_name(app_name);
                }
                if let Some(app_url) = resolve_provider_app_url(provider_name, provider_config) {
                    provider = provider.with_app_url(app_url);
                }
                providers.insert(provider_name.clone(), Arc::new(provider));
            }
            _ => {}
        }
    }

    if providers.is_empty()
        && let Ok(api_key) = std::env::var("OPENROUTER_API_KEY")
        && !api_key.trim().is_empty()
    {
        providers.insert(
            "openrouter".to_string(),
            Arc::new(OpenRouterProvider::new(api_key)),
        );
    }

    Ok(providers)
}

fn resolve_provider_api_key(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Result<String, zagent_core::Error> {
    let env_key = provider_env_var(provider_name, "API_KEY");
    if let Ok(value) = std::env::var(&env_key)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(custom_env_name) = provider_config.api_key_env.as_deref()
        && let Ok(value) = std::env::var(custom_env_name)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    if let Some(raw) = provider_config.api_key.as_deref()
        && !raw.trim().is_empty()
    {
        return Ok(raw.to_string());
    }
    if provider_name == "openrouter"
        && let Ok(value) = std::env::var("OPENROUTER_API_KEY")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }

    Err(zagent_core::Error::config(format!(
        "Provider '{provider_name}' is missing an API key. Set {} or configure api_key/api_key_env in zagent-config.yaml",
        provider_env_var(provider_name, "API_KEY")
    )))
}

fn resolve_provider_base_url(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "BASE_URL"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.base_url.clone())
}

fn resolve_provider_app_name(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "APP_NAME"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.app_name.clone())
}

fn resolve_provider_app_url(
    provider_name: &str,
    provider_config: &ProviderConfig,
) -> Option<String> {
    std::env::var(provider_env_var(provider_name, "APP_URL"))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| provider_config.app_url.clone())
}

fn provider_env_var(provider_name: &str, suffix: &str) -> String {
    let normalized: String = provider_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ZAGENT_PROVIDER_{normalized}_{suffix}")
}

fn select_initial_provider(
    options: &BackendOptions,
    app_config: &zagent_core::config::ZagentConfig,
    providers: &HashMap<String, Arc<dyn Provider>>,
) -> Result<String, zagent_core::Error> {
    if let Ok(from_env) = std::env::var("ZAGENT_DEFAULT_PROVIDER")
        && providers.contains_key(from_env.trim())
    {
        return Ok(from_env.trim().to_string());
    }
    if let Some(from_config) = app_config.default_provider.as_deref()
        && providers.contains_key(from_config.trim())
    {
        return Ok(from_config.trim().to_string());
    }

    if let Some(model) = options.model.as_deref()
        && model.trim().contains(':')
    {
        let mut split = model.trim().splitn(2, ':');
        if let (Some(provider_name), Some(_)) = (split.next(), split.next())
            && providers.contains_key(provider_name)
        {
            return Ok(provider_name.to_string());
        }
    }

    if providers.contains_key("openrouter") {
        return Ok("openrouter".to_string());
    }

    let mut names: Vec<String> = providers.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .next()
        .ok_or_else(|| zagent_core::Error::config("No providers configured"))
}

fn resolve_default_model(
    provider_name: &str,
    app_config: &zagent_core::config::ZagentConfig,
) -> Result<String, zagent_core::Error> {
    if let Ok(model) = std::env::var("ZAGENT_DEFAULT_MODEL")
        && !model.trim().is_empty()
    {
        return Ok(model);
    }
    if let Ok(model) = std::env::var(provider_env_var(provider_name, "DEFAULT_MODEL"))
        && !model.trim().is_empty()
    {
        return Ok(model);
    }
    if let Some(model) = app_config
        .providers
        .get(provider_name)
        .and_then(|p| p.default_model.clone())
        .filter(|m| !m.trim().is_empty())
    {
        return Ok(model);
    }
    if let Some(model) = app_config
        .default_model
        .clone()
        .filter(|m| !m.trim().is_empty())
    {
        return Ok(model);
    }
    if provider_name == "openrouter" {
        return Ok("anthropic/claude-sonnet-4".to_string());
    }
    Err(zagent_core::Error::config(format!(
        "No default model configured for provider '{provider_name}'. Set ZAGENT_DEFAULT_MODEL or configure default_model in zagent-config.yaml"
    )))
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

fn dirs_session_default() -> String {
    format!("{}/.zagent", dirs_home())
}
