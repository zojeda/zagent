use axum::{
    Json, Router,
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;
use tokio::sync::{Mutex, broadcast};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use tower_http::cors::CorsLayer;

use crate::engine::{
    BackendEngine, BackendMessageResponse, BackendSnapshot, McpStatusSnapshot,
    ModelCatalogSnapshot, ModelEventDetailsSnapshot, UiEvent,
};
use crate::migrations::MigrationStatus;
use crate::shell_stream;
use crate::whisper::{TranscribeRequest, TranscribeResponse};
use zagent_core::session::SessionEvent;

#[derive(Debug, Deserialize)]
pub struct MessageRequest {
    pub input: String,
}

#[derive(Debug, Deserialize)]
pub struct StreamMessageQuery {
    pub input: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelEventsQuery {
    pub session_id: Option<String>,
    pub limit: Option<usize>,
    pub sequence: Option<u64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionUiEventsQuery {
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ModelRequest {
    pub provider: Option<String>,
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct ContinueSessionRequest {
    pub name_or_id: String,
}

#[derive(Debug, Deserialize)]
pub struct NewSessionRequest {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RuntimeRequest {
    pub runtime: String,
}

#[derive(Debug, Deserialize)]
pub struct MigrationRequest {
    pub target: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SessionUiEventsSnapshot {
    pub session_id: String,
    pub events: Vec<UiEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StreamChunk {
    pub seq: Option<u64>,
    pub kind: String,
    pub text: Option<String>,
    pub message: Option<String>,
    pub event: Option<UiEvent>,
    pub response: Option<BackendMessageResponse>,
    pub snapshot: Option<BackendSnapshot>,
    pub submitted: Option<String>,
}

#[derive(Clone)]
pub struct ApiState {
    engine: BackendEngine,
    stream_hub: Arc<StreamHub>,
}

impl ApiState {
    fn new(engine: BackendEngine) -> Self {
        Self {
            engine,
            stream_hub: Arc::new(StreamHub::new()),
        }
    }
}

#[derive(Default)]
struct StreamHubState {
    next_seq: u64,
    history: Vec<StreamChunk>,
}

struct StreamHub {
    state: Mutex<StreamHubState>,
    run_lock: Mutex<()>,
    tx: broadcast::Sender<StreamChunk>,
}

impl StreamHub {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            state: Mutex::new(StreamHubState::default()),
            run_lock: Mutex::new(()),
            tx,
        }
    }

    async fn reset_history(&self) {
        let mut state = self.state.lock().await;
        state.history.clear();
    }

    async fn publish(&self, mut chunk: StreamChunk, persist: bool) {
        let mut state = self.state.lock().await;
        chunk.seq = Some(state.next_seq);
        state.next_seq = state.next_seq.saturating_add(1);
        if persist {
            state.history.push(chunk.clone());
            if state.history.len() > 4096 {
                let drain = state.history.len().saturating_sub(4096);
                state.history.drain(0..drain);
            }
        }
        drop(state);
        let _ = self.tx.send(chunk);
    }

    async fn snapshot_history_since(&self, last_seq: u64) -> Vec<StreamChunk> {
        self.state
            .lock()
            .await
            .history
            .iter()
            .filter(|c| c.seq.unwrap_or(0) > last_seq)
            .cloned()
            .collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<StreamChunk> {
        self.tx.subscribe()
    }
}

pub fn router(engine: BackendEngine) -> Router {
    let state = ApiState::new(engine);
    Router::new()
        .route("/api/health", get(health))
        .route("/api/state", get(get_state))
        .route("/api/sessions", get(list_sessions))
        .route("/api/session/ui-events", get(get_session_ui_events))
        .route("/api/session/model-events", get(get_model_events))
        .route("/api/message", post(post_message))
        .route("/api/message/start", post(start_message))
        .route("/api/transcribe", post(post_transcribe))
        .route("/api/events/stream", get(get_events_stream))
        .route("/api/models", get(get_models))
        .route("/api/migrations", get(get_migrations_status))
        .route("/api/migrations/up", post(run_migrations_up))
        .route("/api/migrations/down", post(run_migrations_down))
        .route("/api/mcp", get(get_mcp_status))
        .route(
            "/api/message/stream",
            get(get_message_stream).post(post_message_stream),
        )
        .route("/api/model", post(set_model))
        .route("/api/runtime", post(set_runtime))
        .route("/api/session/new", post(new_session))
        .route("/api/session/continue", post(continue_session))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

async fn health() -> &'static str {
    "ok"
}

async fn get_state(State(state): State<ApiState>) -> Json<BackendSnapshot> {
    Json(state.engine.snapshot().await)
}

async fn get_models(
    State(state): State<ApiState>,
) -> Result<Json<ModelCatalogSnapshot>, (axum::http::StatusCode, Json<ApiError>)> {
    match state.engine.list_models_catalog().await {
        Ok(catalog) => Ok(Json(catalog)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn get_mcp_status(
    State(state): State<ApiState>,
) -> Result<Json<McpStatusSnapshot>, (axum::http::StatusCode, Json<ApiError>)> {
    match state.engine.mcp_status_snapshot().await {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn list_sessions(
    State(state): State<ApiState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<ApiError>)> {
    match state.engine.list_sessions().await {
        Ok(sessions) => Ok(Json(serde_json::json!({ "sessions": sessions }))),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn get_model_events(
    State(state): State<ApiState>,
    Query(query): Query<ModelEventsQuery>,
) -> Result<Json<ModelEventDetailsSnapshot>, (axum::http::StatusCode, Json<ApiError>)> {
    match state
        .engine
        .model_event_details(
            query.session_id.as_deref(),
            query.limit,
            query.sequence,
            query.event_id.as_deref(),
        )
        .await
    {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn get_session_ui_events(
    State(state): State<ApiState>,
    Query(query): Query<SessionUiEventsQuery>,
) -> Result<Json<SessionUiEventsSnapshot>, (axum::http::StatusCode, Json<ApiError>)> {
    let resolved_session_id = query.session_id.clone().unwrap_or_default();
    match state
        .engine
        .session_events(query.session_id.as_deref())
        .await
    {
        Ok(events) => Ok(Json(SessionUiEventsSnapshot {
            session_id: resolved_session_id,
            events: events
                .into_iter()
                .filter_map(session_event_to_ui_event)
                .collect(),
        })),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn get_migrations_status(
    State(state): State<ApiState>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<ApiError>)> {
    match state.engine.migration_status().await {
        Ok(migrations) => Ok(Json(serde_json::json!({ "migrations": migrations }))),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn run_migrations_up(
    State(state): State<ApiState>,
    Json(req): Json<MigrationRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<ApiError>)> {
    let result = if let Some(target) = req.target {
        state.engine.migrate_to(target).await
    } else {
        state.engine.migrate_to_latest().await
    };

    match result {
        Ok(version) => {
            let migrations: Vec<MigrationStatus> =
                state.engine.migration_status().await.unwrap_or_default();
            Ok(Json(serde_json::json!({
                "ok": true,
                "current_version": version,
                "migrations": migrations
            })))
        }
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn run_migrations_down(
    State(state): State<ApiState>,
    Json(req): Json<MigrationRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<ApiError>)> {
    let target = req.target.unwrap_or(0);
    match state.engine.migrate_to(target).await {
        Ok(version) => {
            let migrations: Vec<MigrationStatus> =
                state.engine.migration_status().await.unwrap_or_default();
            Ok(Json(serde_json::json!({
                "ok": true,
                "current_version": version,
                "migrations": migrations
            })))
        }
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn post_message(
    State(state): State<ApiState>,
    Json(req): Json<MessageRequest>,
) -> Result<Json<BackendMessageResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    match run_message_turn(state, req.input).await {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn start_message(
    State(state): State<ApiState>,
    Json(req): Json<MessageRequest>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, Json<ApiError>)> {
    tokio::spawn(async move {
        let _ = run_message_turn(state, req.input).await;
    });
    Ok(axum::http::StatusCode::ACCEPTED)
}

async fn post_transcribe(
    Json(req): Json<TranscribeRequest>,
) -> Result<Json<TranscribeResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    match crate::whisper::transcribe_wav(req).await {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn post_message_stream(
    State(state): State<ApiState>,
    Json(req): Json<MessageRequest>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    build_message_stream(state, req.input)
}

async fn get_message_stream(
    State(state): State<ApiState>,
    Query(query): Query<StreamMessageQuery>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    build_message_stream(state, query.input)
}

async fn get_events_stream(
    State(state): State<ApiState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    build_message_stream(state, String::new())
}

fn build_message_stream(
    state: ApiState,
    input: String,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let (tx_client, rx_client) = tokio::sync::mpsc::unbounded_channel::<StreamChunk>();
    let state_for_stream = state.clone();

    tokio::spawn(async move {
        if !input.trim().is_empty() {
            let turn_state = state_for_stream.clone();
            tokio::spawn(async move {
                let _ = run_message_turn(turn_state, input).await;
            });
        }

        let snapshot = state_for_stream.engine.snapshot().await;
        if tx_client
            .send(StreamChunk {
                seq: None,
                kind: "snapshot".to_string(),
                text: None,
                message: None,
                event: None,
                response: None,
                snapshot: Some(snapshot),
                submitted: None,
            })
            .is_err()
        {
            return;
        }

        let mut rx = state_for_stream.stream_hub.subscribe();
        let mut last_seq = 0u64;

        for chunk in state_for_stream
            .stream_hub
            .snapshot_history_since(last_seq)
            .await
        {
            last_seq = last_seq.max(chunk.seq.unwrap_or(0));
            if tx_client.send(chunk).is_err() {
                return;
            }
        }

        loop {
            match rx.recv().await {
                Ok(chunk) => {
                    let seq = chunk.seq.unwrap_or(0);
                    if seq <= last_seq {
                        continue;
                    }
                    last_seq = seq;
                    if tx_client.send(chunk).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    for chunk in state_for_stream
                        .stream_hub
                        .snapshot_history_since(last_seq)
                        .await
                    {
                        last_seq = last_seq.max(chunk.seq.unwrap_or(0));
                        if tx_client.send(chunk).is_err() {
                            return;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let stream = UnboundedReceiverStream::new(rx_client).map(|chunk| {
        let data = serde_json::to_string(&chunk).unwrap_or_else(|_| {
            "{\"kind\":\"error\",\"message\":\"failed to serialize stream chunk\"}".to_string()
        });
        Ok(Event::default().data(data))
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(1)))
}

async fn run_message_turn(
    state: ApiState,
    input: String,
) -> Result<BackendMessageResponse, zagent_core::Error> {
    let _run_guard = state.stream_hub.run_lock.lock().await;
    state.stream_hub.reset_history().await;
    state
        .stream_hub
        .publish(
            StreamChunk {
                seq: None,
                kind: "status".to_string(),
                text: None,
                message: Some("request accepted".to_string()),
                event: None,
                response: None,
                snapshot: None,
                submitted: None,
            },
            true,
        )
        .await;
    state
        .stream_hub
        .publish(
            StreamChunk {
                seq: None,
                kind: "submit".to_string(),
                text: None,
                message: None,
                event: None,
                response: None,
                snapshot: None,
                submitted: Some(input.clone()),
            },
            true,
        )
        .await;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
    let mut shell_rx = shell_stream::subscribe();
    let forwarder = state
        .engine
        .subscribe_session_events()
        .await
        .ok()
        .map(|mut live_rx| {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                while let Some(event) = live_rx.recv().await {
                    if let Some(ui_event) = session_event_to_ui_event(event) {
                        let _ = event_tx.send(ui_event);
                    }
                }
            })
        });
    let (done_tx, mut done_rx) = oneshot::channel();
    let engine = state.engine.clone();
    let use_live_events = forwarder.is_some();
    let callback_tx = event_tx.clone();
    tokio::spawn(async move {
        let result = if use_live_events {
            engine
                .send_input_with_progress(&input, move |_event| {})
                .await
        } else {
            engine
                .send_input_with_progress(&input, move |event| {
                    let _ = callback_tx.send(event);
                })
                .await
        };
        let _ = done_tx.send(result);
    });

    let mut interval = tokio::time::interval(Duration::from_millis(350));
    let mut events_open = true;
    loop {
        tokio::select! {
            _ = interval.tick() => {
                state.stream_hub.publish(StreamChunk {
                    seq: None,
                    kind: "heartbeat".to_string(),
                    text: None,
                    message: Some("model thinking...".to_string()),
                    event: None,
                    response: None,
                    snapshot: None,
                    submitted: None,
                }, false).await;
            }
            maybe_event = event_rx.recv(), if events_open => {
                if let Some(event) = maybe_event {
                    state.stream_hub.publish(StreamChunk {
                        seq: None,
                        kind: "event".to_string(),
                        text: None,
                        message: None,
                        event: Some(event),
                        response: None,
                        snapshot: None,
                        submitted: None,
                    }, true).await;
                } else {
                    events_open = false;
                }
            }
            shell_chunk = shell_rx.recv() => {
                match shell_chunk {
                    Ok(chunk) => {
                        state.stream_hub.publish(StreamChunk {
                            seq: None,
                            kind: "event".to_string(),
                            text: None,
                            message: None,
                            event: Some(UiEvent {
                                kind: "tool_stream".to_string(),
                                title: format!("shell {}", chunk.channel),
                                detail: format!("stream {}", chunk.channel),
                                payload: Some(serde_json::json!({
                                    "stream_id": chunk.stream_id,
                                    "tool_call_id": chunk.stream_id,
                                    "channel": chunk.channel,
                                    "text": chunk.text,
                                })),
                            }),
                            response: None,
                            snapshot: None,
                            submitted: None,
                        }, true).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
            }
            res = &mut done_rx => {
                // Live DB notifications can lag slightly behind turn completion.
                // Give the forwarder a brief window to push final events, then drain
                // any queued UI events before publishing `final`.
                tokio::time::sleep(Duration::from_millis(120)).await;
                if let Some(handle) = forwarder.as_ref() {
                    handle.abort();
                }
                loop {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            state.stream_hub.publish(StreamChunk {
                                seq: None,
                                kind: "event".to_string(),
                                text: None,
                                message: None,
                                event: Some(event),
                                response: None,
                                snapshot: None,
                                submitted: None,
                            }, true).await;
                        }
                        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                    }
                }
                return match res {
                    Ok(Ok(resp)) => {
                        if resp.kind == "assistant" && !resp.output.is_empty() {
                            for chunk in split_text_chunks(&resp.output, 32) {
                                state.stream_hub.publish(StreamChunk {
                                    seq: None,
                                    kind: "delta".to_string(),
                                    text: Some(chunk),
                                    message: None,
                                    event: None,
                                    response: None,
                                    snapshot: None,
                                    submitted: None,
                                }, true).await;
                                tokio::time::sleep(Duration::from_millis(18)).await;
                            }
                        }
                        state.stream_hub.publish(StreamChunk {
                            seq: None,
                            kind: "final".to_string(),
                            text: None,
                            message: Some("done".to_string()),
                            event: None,
                            response: Some(resp.clone()),
                            snapshot: None,
                            submitted: None,
                        }, true).await;
                        // Publish a terminal snapshot so reconnecting or lagged clients can
                        // reconcile to the fully committed session state even if they miss `final`.
                        state.stream_hub.publish(StreamChunk {
                            seq: None,
                            kind: "snapshot".to_string(),
                            text: None,
                            message: None,
                            event: None,
                            response: None,
                            snapshot: Some(resp.state.clone()),
                            submitted: None,
                        }, true).await;
                        Ok(resp)
                    }
                    Ok(Err(err)) => {
                        state.stream_hub.publish(StreamChunk {
                            seq: None,
                            kind: "error".to_string(),
                            text: None,
                            message: Some(err.to_string()),
                            event: None,
                            response: None,
                            snapshot: None,
                            submitted: None,
                        }, true).await;
                        Err(err)
                    }
                    Err(err) => {
                        let msg = format!("stream channel error: {err}");
                        state.stream_hub.publish(StreamChunk {
                            seq: None,
                            kind: "error".to_string(),
                            text: None,
                            message: Some(msg.clone()),
                            event: None,
                            response: None,
                            snapshot: None,
                            submitted: None,
                        }, true).await;
                        Err(zagent_core::Error::provider("stream", msg))
                    }
                };
            }
        }
    }
}

fn session_event_to_ui_event(event: SessionEvent) -> Option<UiEvent> {
    let SessionEvent {
        id: event_id,
        session_id: _,
        session_ref: _,
        sequence,
        parent_event_id: _,
        parent_event_ref: _,
        kind,
        phase: _,
        agent,
        handoff_depth,
        turn,
        provider,
        provider_ref: _,
        model,
        model_ref: _,
        tool_name,
        tool_call_id,
        success,
        finish_reason,
        latency_ms,
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cached_prompt_tokens,
        cost_usd,
        credits_remaining,
        arguments,
        result,
        payload,
        created_at: _,
    } = event;

    match kind.as_str() {
        "model_request_started" => {
            let provider = provider
                .as_deref()
                .or_else(|| payload.get("provider").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            let model = model
                .as_deref()
                .or_else(|| payload.get("model").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            let message_count = payload
                .get("message_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tool_count = payload
                .get("tool_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Some(UiEvent {
                kind: "model".to_string(),
                title: "model request".to_string(),
                detail: format!(
                    "turn={} provider={} model={} msgs={} tools={}",
                    turn.unwrap_or(0),
                    provider,
                    model,
                    message_count,
                    tool_count,
                ),
                payload: Some(serde_json::json!({
                    "event_id": event_id,
                    "sequence": sequence,
                    "phase": "request_started",
                    "agent": agent,
                    "handoff_depth": handoff_depth,
                    "turn": turn,
                    "provider": provider,
                    "model": model,
                    "message_count": message_count,
                    "tool_count": tool_count
                })),
            })
        }
        "model_response_received" => {
            let finish_reason = finish_reason
                .as_deref()
                .or_else(|| payload.get("finish_reason").and_then(|v| v.as_str()));
            let latency_ms = latency_ms
                .or_else(|| payload.get("latency_ms").and_then(|v| v.as_u64()))
                .unwrap_or(0);
            let prompt_tokens = serde_json::to_value(
                prompt_tokens.or_else(|| payload.get("prompt_tokens").and_then(|v| v.as_u64())),
            )
            .unwrap_or(serde_json::Value::Null);
            let completion_tokens = serde_json::to_value(
                completion_tokens
                    .or_else(|| payload.get("completion_tokens").and_then(|v| v.as_u64())),
            )
            .unwrap_or(serde_json::Value::Null);
            let total_tokens = serde_json::to_value(
                total_tokens.or_else(|| payload.get("total_tokens").and_then(|v| v.as_u64())),
            )
            .unwrap_or(serde_json::Value::Null);
            let cached_prompt_tokens = serde_json::to_value(
                cached_prompt_tokens
                    .or_else(|| payload.get("cached_prompt_tokens").and_then(|v| v.as_u64())),
            )
            .unwrap_or(serde_json::Value::Null);
            let cost_usd = serde_json::to_value(
                cost_usd.or_else(|| payload.get("cost_usd").and_then(|v| v.as_f64())),
            )
            .unwrap_or(serde_json::Value::Null);
            let credits_remaining = serde_json::to_value(
                credits_remaining
                    .or_else(|| payload.get("credits_remaining").and_then(|v| v.as_f64())),
            )
            .unwrap_or(serde_json::Value::Null);
            Some(UiEvent {
                kind: "model".to_string(),
                title: "model response".to_string(),
                detail: format!(
                    "turn={} latency={}ms finish={}",
                    turn.unwrap_or(0),
                    latency_ms,
                    finish_reason.unwrap_or("unknown")
                ),
                payload: Some(serde_json::json!({
                    "event_id": event_id,
                    "sequence": sequence,
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
            })
        }
        "tool_call_started" => {
            let tool_name = tool_name
                .as_deref()
                .or_else(|| payload.get("tool_name").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            let tool_call_id = tool_call_id
                .as_deref()
                .or_else(|| payload.get("tool_call_id").and_then(|v| v.as_str()));
            let arguments = arguments
                .as_ref()
                .map(|v| serde_json::Value::String(v.clone()))
                .or_else(|| payload.get("arguments").cloned())
                .unwrap_or(serde_json::Value::Null);
            Some(UiEvent {
                kind: "tool".to_string(),
                title: format!("tool: {tool_name}"),
                detail: "started".to_string(),
                payload: Some(serde_json::json!({
                    "event_id": event_id,
                    "sequence": sequence,
                    "phase": "start",
                    "agent": agent,
                    "handoff_depth": handoff_depth,
                    "turn": turn,
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "arguments": arguments
                })),
            })
        }
        "tool_call_finished" => {
            let tool_name = tool_name
                .as_deref()
                .or_else(|| payload.get("tool_name").and_then(|v| v.as_str()))
                .unwrap_or("unknown");
            let success = success
                .or_else(|| payload.get("success").and_then(|v| v.as_bool()))
                .unwrap_or(false);
            let latency_ms = latency_ms
                .or_else(|| payload.get("latency_ms").and_then(|v| v.as_u64()))
                .unwrap_or(0);
            let tool_call_id = tool_call_id
                .as_deref()
                .or_else(|| payload.get("tool_call_id").and_then(|v| v.as_str()));
            let result = result
                .as_ref()
                .map(|v| serde_json::Value::String(v.clone()))
                .or_else(|| payload.get("result").cloned())
                .unwrap_or(serde_json::Value::Null);
            Some(UiEvent {
                kind: "tool".to_string(),
                title: format!("tool: {tool_name}"),
                detail: if success {
                    format!("ok in {latency_ms}ms")
                } else {
                    format!("failed in {latency_ms}ms")
                },
                payload: Some(serde_json::json!({
                    "event_id": event_id,
                    "sequence": sequence,
                    "phase": "finish",
                    "agent": agent,
                    "handoff_depth": handoff_depth,
                    "turn": turn,
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "success": success,
                    "latency_ms": latency_ms,
                    "result": result
                })),
            })
        }
        _ => None,
    }
}

fn split_text_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if current.chars().count() >= max_chars {
            chunks.push(current);
            current = String::new();
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

async fn set_model(
    State(state): State<ApiState>,
    Json(req): Json<ModelRequest>,
) -> Result<Json<BackendMessageResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    match state
        .engine
        .set_model(req.provider.as_deref(), &req.model)
        .await
    {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn set_runtime(
    State(state): State<ApiState>,
    Json(req): Json<RuntimeRequest>,
) -> Result<Json<BackendMessageResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    let command = format!("/runtime {}", req.runtime);
    match state.engine.send_input(&command).await {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn new_session(
    State(state): State<ApiState>,
    Json(req): Json<NewSessionRequest>,
) -> Result<Json<BackendMessageResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    let command = if let Some(name) = req.name {
        format!("/session new {}", name)
    } else {
        "/session new".to_string()
    };
    match state.engine.send_input(&command).await {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}

async fn continue_session(
    State(state): State<ApiState>,
    Json(req): Json<ContinueSessionRequest>,
) -> Result<Json<BackendMessageResponse>, (axum::http::StatusCode, Json<ApiError>)> {
    let command = format!("/session continue {}", req.name_or_id);
    match state.engine.send_input(&command).await {
        Ok(resp) => Ok(Json(resp)),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )),
    }
}
