use api::{
    normalize_base_url, BackendSnapshot, ConversationLine, ModelEventDetail,
    ModelEventDetailsSnapshot, ProxyClient, StreamChunk, UiEvent,
};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use ui::{AgentNodeView, ChatTurnView, Dashboard, FooterTotalsView, ServerTabView};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

const MAIN_CSS: Asset = asset!("/assets/main.css");
const HLJS_THEME: &str =
    "https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.1/styles/github.min.css";
const HLJS_SCRIPT: &str =
    "https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.1/highlight.min.js";
const STORAGE_KEY: &str = "zagent.dioxus.servers.v1";
const MERMAID_CSS: &str = "https://cdnjs.cloudflare.com/ajax/libs/mermaid/11.4.1/mermaid.min.css";
const MERMAID_JS: &str = "https://cdnjs.cloudflare.com/ajax/libs/mermaid/11.4.1/mermaid.min.js";
const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";

#[derive(Debug, Clone)]
struct AppState {
    active_tab_id: usize,
    next_server_id: usize,
    show_connect_form: bool,
    connect_value: String,
    servers: Vec<ServerSession>,
}

#[derive(Debug, Clone)]
struct ServerSession {
    id: usize,
    label: String,
    client: ProxyClient,
    prompt: String,
    pending: bool,
    connected: bool,
    status_text: String,
    model: String,
    session_id: String,
    turns: Vec<ChatTurn>,
    next_turn_id: usize,
    active_response_turn_id: Option<usize>,
    suppress_replayed_loop: bool,
    conversation_follow: bool,
    conversation_unseen: usize,
    model_json_dialog_open: bool,
    model_json_loading: bool,
    model_json_error: Option<String>,
    model_json_session_id: String,
    model_json_events: Vec<ModelEventDetail>,
    nodes: Vec<AgentNode>,
    pending_handoffs: Vec<PendingHandoff>,
    lineage: Vec<Option<usize>>,
    next_node_id: usize,
    totals: FooterTotalsView,
}

#[derive(Debug, Clone)]
struct ChatTurn {
    id: usize,
    author: String,
    role_class: String,
    markdown: String,
    agent_roots: Vec<AgentNodeView>,
    agent_details_collapsed: bool,
    agent_details_enabled: bool,
}

#[derive(Debug, Clone)]
struct AgentNode {
    id: usize,
    parent_id: Option<usize>,
    name: String,
    depth: u32,
    phase_badge: String,
    sent: u64,
    received: u64,
    cache: u64,
    context_pct: f64,
    cost_usd: f64,
    reasoning_text: Option<String>,
    handoff_message: Option<String>,
    handoff_context: Option<String>,
    last_event: String,
    timeline: Vec<AgentTimelineEntry>,
}

#[derive(Debug, Clone)]
struct AgentTimelineEntry {
    text: String,
    kind: String,
    phase: String,
    sequence: Option<u64>,
    event_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingHandoff {
    parent_id: usize,
    depth: u32,
    message: String,
    context: Option<String>,
}

struct StreamHandle {
    server_id: usize,
    source: web_sys::EventSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedTabs {
    urls: Vec<String>,
    active_url: Option<String>,
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut app = use_signal(initial_state);
    let streams = use_signal(|| Vec::<StreamHandle>::new());
    let mut booted = use_signal(|| false);

    if !booted() {
        booted.set(true);
        let ids = app().servers.iter().map(|s| s.id).collect::<Vec<_>>();
        for id in ids {
            open_server_connection(app, streams, id);
        }
    }

    let current = current_server(&app());
    let tabs = app()
        .servers
        .iter()
        .map(|s| ServerTabView {
            id: s.id,
            label: s.label.clone(),
            connected: s.connected,
        })
        .collect::<Vec<_>>();

    use_effect(move || {
        let state = app();
        if let Some(server) = state.servers.iter().find(|s| s.id == state.active_tab_id) {
            follow_latest_events(server.conversation_follow);
        }
        highlight_markdown_code();
        render_mermaid_diagrams();
    });

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: HLJS_THEME }
        document::Script { src: HLJS_SCRIPT }
        document::Stylesheet { href: MERMAID_CSS }
        document::Script { src: MERMAID_JS }
        Dashboard {
            title: "zAgent Dioxus".to_string(),
            status_text: current
                .as_ref()
                .map(|s| s.status_text.clone())
                .unwrap_or_else(|| "No server selected".to_string()),
            connected: current.as_ref().is_some_and(|s| s.connected),
            pending: current.as_ref().is_some_and(|s| s.pending),
            prompt_value: current
                .as_ref()
                .map(|s| s.prompt.clone())
                .unwrap_or_default(),
            turns: current
                .as_ref()
                .map(build_chat_turns)
                .unwrap_or_default(),
            totals: current
                .as_ref()
                .map(|s| s.totals.clone())
                .unwrap_or_default(),
            tabs,
            active_tab_id: app().active_tab_id,
            show_connect_form: app().show_connect_form,
            connect_value: app().connect_value.clone(),
            on_select_tab: move |tab_id| {
                let mut state = app.write();
                state.active_tab_id = tab_id;
                if let Some(server) = current_server_mut(&mut state) {
                    server.conversation_unseen = 0;
                }
                persist_tabs(&state);
            },
            on_close_tab: move |tab_id| {
                close_stream_for_server(streams, tab_id);
                let mut state = app.write();
                if state.servers.len() <= 1 {
                    return;
                }
                if let Some(pos) = state.servers.iter().position(|s| s.id == tab_id) {
                    state.servers.remove(pos);
                    if state.active_tab_id == tab_id {
                        state.active_tab_id = state.servers.first().map(|s| s.id).unwrap_or(0);
                    }
                }
                persist_tabs(&state);
            },
            on_reconnect_tab: move |tab_id| {
                open_server_connection(app, streams, tab_id);
            },
            on_toggle_add_server: move |_| {
                let mut state = app.write();
                state.show_connect_form = !state.show_connect_form;
                if state.connect_value.is_empty() {
                    state.connect_value = DEFAULT_SERVER_URL.to_string();
                }
            },
            on_connect_input: move |value| {
                app.write().connect_value = value;
            },
            on_connect_submit: move |_| {
                let url = normalize_base_url(&app().connect_value);
                if url.is_empty() {
                    return;
                }

                if let Some(existing_id) = app()
                    .servers
                    .iter()
                    .find(|s| s.client.base_url == url)
                    .map(|s| s.id)
                {
                    let mut state = app.write();
                    state.active_tab_id = existing_id;
                    state.show_connect_form = false;
                    persist_tabs(&state);
                    return;
                }

                let id = app().next_server_id;
                {
                    let mut state = app.write();
                    state.next_server_id = state.next_server_id.saturating_add(1);
                    state.active_tab_id = id;
                    state.show_connect_form = false;
                    state.servers.push(new_server_session(id, &url));
                    persist_tabs(&state);
                }
                open_server_connection(app, streams, id);
            },
            on_prompt_input: move |value| {
                if let Some(server) = current_server_mut(&mut app.write()) {
                    server.prompt = value;
                }
            },
            on_submit: move |_| {
                let (server_id, client, text) = {
                    let mut state = app.write();
                    let Some(server) = current_server_mut(&mut state) else {
                        return;
                    };
                    let text = server.prompt.trim().to_string();
                    if text.is_empty() || server.pending {
                        return;
                    }
                    server.pending = true;
                    server.status_text = "message accepted".to_string();
                    server.conversation_follow = true;
                    server.conversation_unseen = 0;
                    append_user_turn_if_new(server, &text);
                    begin_streaming_response(server);
                    server.prompt.clear();
                    (server.id, server.client.clone(), text)
                };

                spawn(async move {
                    if let Err(err) = client.start_message(&text).await {
                        let mut state = app.write();
                        if let Some(server) = find_server_mut_by_id(&mut state, server_id) {
                            server.pending = false;
                            server.status_text = err;
                        }
                    }
                });
            },
            on_toggle_turn_details: move |turn_id| {
                if let Some(server) = current_server_mut(&mut app.write()) {
                    if let Some(turn) = server.turns.iter_mut().find(|t| t.id == turn_id) {
                        if turn.agent_roots.is_empty() {
                            return;
                        }
                        turn.agent_details_collapsed = !turn.agent_details_collapsed;
                        turn.agent_details_enabled = true;
                    }
                }
            },
            on_conversation_scroll: move |_| {
                let mut state = app.write();
                let Some(server) = current_server_mut(&mut state) else {
                    return;
                };
                let Some((scroll_top, scroll_height, client_height)) = conversation_scroll_metrics()
                else {
                    return;
                };
                let near_bottom = is_near_bottom(scroll_top, scroll_height, client_height);
                if near_bottom {
                    server.conversation_follow = true;
                    server.conversation_unseen = 0;
                } else {
                    server.conversation_follow = false;
                }
            },
            show_resume_autoscroll: current.as_ref().is_some_and(|s| !s.conversation_follow),
            resume_autoscroll_label: current
                .as_ref()
                .map(|s| {
                    if s.conversation_unseen == 0 {
                        "Resume auto-scroll".to_string()
                    } else {
                        format!("Resume auto-scroll ({} new updates)", s.conversation_unseen)
                    }
                })
                .unwrap_or_else(|| "Resume auto-scroll".to_string()),
            on_resume_autoscroll: move |_| {
                if let Some(server) = current_server_mut(&mut app.write()) {
                    server.conversation_follow = true;
                    server.conversation_unseen = 0;
                }
                follow_latest_events(true);
            },
            on_open_model_event: move |event_id: String| {
                let (server_id, client, session_id) = {
                    let mut state = app.write();
                    let Some(server) = current_server_mut(&mut state) else {
                        return;
                    };
                    server.model_json_dialog_open = true;
                    server.model_json_loading = true;
                    server.model_json_error = None;
                    server.model_json_events.clear();
                    server.model_json_session_id.clear();
                    (server.id, server.client.clone(), server.session_id.clone())
                };

                spawn(async move {
                    let result = client
                        .fetch_model_event_details(
                            Some(session_id.as_str()),
                            Some(1),
                            None,
                            Some(event_id.as_str()),
                        )
                        .await;
                    if let Some(server) = find_server_mut_by_id(&mut app.write(), server_id) {
                        server.model_json_loading = false;
                        match result {
                            Ok(snapshot) => apply_model_event_snapshot(server, snapshot),
                            Err(err) => server.model_json_error = Some(err),
                        }
                    }
                });
            }
        }
        if current
            .as_ref()
            .is_some_and(|server| server.model_json_dialog_open)
        {
            div { class: "json-modal-backdrop",
                div { class: "json-modal",
                    div { class: "json-modal-header",
                        h3 { "Event Details JSON" }
                        button {
                            class: "json-modal-close",
                            onclick: move |_| {
                                if let Some(server) = current_server_mut(&mut app.write()) {
                                    server.model_json_dialog_open = false;
                                }
                            },
                            "Close"
                        }
                    }
                    if let Some(server) = current.as_ref() {
                        if server.model_json_loading {
                            p { class: "json-modal-status", "Loading event details..." }
                        } else if let Some(err) = &server.model_json_error {
                            p { class: "json-modal-error", "{err}" }
                        } else {
                            p { class: "json-modal-status", "Session: {server.model_json_session_id}" }
                            if server.model_json_events.is_empty() {
                                p { class: "json-modal-status", "No event details available." }
                            } else {
                                div { class: "json-modal-list",
                                    for event in &server.model_json_events {
                                        article { class: "json-modal-event",
                                            h4 {
                                                "#{event.sequence.unwrap_or(0)} {event.kind} agent={event.agent} turn={event.turn.unwrap_or(0)}"
                                            }
                                            p {
                                                class: "json-modal-meta",
                                                "provider={event.provider.clone().unwrap_or_else(|| \"-\".to_string())} model={event.model.clone().unwrap_or_else(|| \"-\".to_string())}"
                                            }
                                            p { class: "json-modal-meta", "{event.created_at}" }
                                            pre { class: "json-modal-pre", "{pretty_json_value(event.json_detail.as_ref())}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn follow_latest_events(follow: bool) {
    if !follow {
        return;
    }
    let document = web_sys::window().and_then(|w| w.document());
    let Some(el) = document.and_then(|d| d.get_element_by_id("conversation-scroll")) else {
        return;
    };
    let Ok(content) = el.dyn_into::<web_sys::HtmlElement>() else {
        return;
    };
    content.set_scroll_top(content.scroll_height());
}

fn conversation_scroll_metrics() -> Option<(i32, i32, i32)> {
    let document = web_sys::window().and_then(|w| w.document())?;
    let el = document.get_element_by_id("conversation-scroll")?;
    let content = el.dyn_into::<web_sys::HtmlElement>().ok()?;
    Some((
        content.scroll_top(),
        content.scroll_height(),
        content.client_height(),
    ))
}

fn is_near_bottom(scroll_top: i32, scroll_height: i32, client_height: i32) -> bool {
    (scroll_height - (scroll_top + client_height)).abs() <= 16
}

fn highlight_markdown_code() {
    let _ = js_sys::eval(
        "if (window.hljs) { document.querySelectorAll('.markdown-body code').forEach((el) => { if (!el.classList.contains('hljs') && !el.dataset.highlighted) { window.hljs.highlightElement(el); } }); }",
    );
}

fn render_mermaid_diagrams() {
    // Initialize mermaid if not already done, then render any mermaid diagrams
    let _ = js_sys::eval(
        "if (window.mermaid) { \
            if (!window.mermaidInitialized) { \
                window.mermaid.initialize({ startOnLoad: false }); \
                window.mermaidInitialized = true; \
            } \
            window.mermaid.run({ querySelector: '.mermaid' }); \
        }",
    );
}

fn initial_state() -> AppState {
    let origin = current_origin();
    let persisted = load_persisted_tabs();

    let mut urls = persisted
        .as_ref()
        .map(|p| {
            if p.urls.is_empty() {
                default_server_urls()
            } else {
                p.urls
                    .iter()
                    .map(|u| normalize_base_url(u))
                    .filter(|u| !u.is_empty())
                    .collect::<Vec<_>>()
            }
        })
        .filter(|urls| !urls.is_empty())
        .unwrap_or_else(default_server_urls);

    urls = unique_urls(urls);
    let normalized_origin = normalize_base_url(&origin);
    if normalized_origin != DEFAULT_SERVER_URL {
        urls.retain(|u| u != &normalized_origin);
    }
    if !urls.iter().any(|u| u == DEFAULT_SERVER_URL) {
        urls.insert(0, DEFAULT_SERVER_URL.to_string());
    }

    let mut servers = Vec::new();
    for (id, url) in urls.into_iter().enumerate() {
        servers.push(new_server_session(id, &url));
    }

    let default_active = servers.first().map(|s| s.id).unwrap_or(0);
    let active = persisted
        .and_then(|p| p.active_url)
        .and_then(|url| {
            let target = normalize_base_url(&url);
            servers
                .iter()
                .find(|s| s.client.base_url == target)
                .map(|s| s.id)
        })
        .unwrap_or(default_active);

    AppState {
        active_tab_id: active,
        next_server_id: servers.len(),
        show_connect_form: false,
        connect_value: DEFAULT_SERVER_URL.to_string(),
        servers,
    }
}

fn new_server_session(id: usize, base_url: &str) -> ServerSession {
    let client = ProxyClient::new(base_url.to_string());
    ServerSession {
        id,
        label: server_label(id, &client.base_url),
        client,
        prompt: String::new(),
        pending: false,
        connected: false,
        status_text: "connecting...".to_string(),
        model: String::new(),
        session_id: String::new(),
        turns: Vec::new(),
        next_turn_id: 0,
        active_response_turn_id: None,
        suppress_replayed_loop: false,
        conversation_follow: true,
        conversation_unseen: 0,
        model_json_dialog_open: false,
        model_json_loading: false,
        model_json_error: None,
        model_json_session_id: String::new(),
        model_json_events: Vec::new(),
        nodes: Vec::new(),
        pending_handoffs: Vec::new(),
        lineage: Vec::new(),
        next_node_id: 0,
        totals: FooterTotalsView {
            top_agent: "none".to_string(),
            ..FooterTotalsView::default()
        },
    }
}

fn server_label(id: usize, base_url: &str) -> String {
    let host = base_url
        .split("//")
        .nth(1)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url);
    format!("S{} {}", id + 1, host)
}

fn open_server_connection(
    mut app: Signal<AppState>,
    mut streams: Signal<Vec<StreamHandle>>,
    server_id: usize,
) {
    let Some(client) = app()
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .map(|s| s.client.clone())
    else {
        return;
    };

    let fetch_client = client.clone();
    spawn(async move {
        match fetch_client.fetch_state().await {
            Ok(snapshot) => {
                let session_id = snapshot.session.id.clone();
                let ui_events = fetch_client
                    .fetch_session_ui_events(Some(session_id.as_str()))
                    .await
                    .ok()
                    .map(|snapshot| snapshot.events)
                    .unwrap_or_default();
                if let Some(server) = find_server_mut_by_id(&mut app.write(), server_id) {
                    handle_snapshot_chunk(server, &snapshot, "snapshot received");
                    hydrate_agent_details_from_ui_events(server, &ui_events);
                }
            }
            Err(err) => {
                if let Some(server) = find_server_mut_by_id(&mut app.write(), server_id) {
                    server.connected = false;
                    server.status_text = err;
                }
            }
        }
    });

    let source = match web_sys::EventSource::new(&client.events_stream_url()) {
        Ok(es) => es,
        Err(err) => {
            if let Some(server) = find_server_mut_by_id(&mut app.write(), server_id) {
                server.connected = false;
                server.status_text = format!("event stream failed: {:?}", err);
            }
            return;
        }
    };

    let source_for_message = source.clone();
    let mut app_for_message = app;
    let on_message = Closure::<dyn FnMut(_)>::new(move |event: web_sys::MessageEvent| {
        let payload = event.data().as_string().unwrap_or_default();
        match serde_json::from_str::<StreamChunk>(&payload) {
            Ok(chunk) => apply_chunk_to_server(&mut app_for_message, server_id, chunk),
            Err(err) => {
                if let Some(server) = find_server_mut_by_id(&mut app_for_message.write(), server_id)
                {
                    server.pending = false;
                    server.connected = false;
                    server.status_text = format!("stream parse error: {err}");
                }
                source_for_message.close();
            }
        }
    });
    source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let mut app_for_error = app;
    let on_error = Closure::<dyn FnMut(_)>::new(move |_event: web_sys::Event| {
        if let Some(server) = find_server_mut_by_id(&mut app_for_error.write(), server_id) {
            server.pending = false;
            server.connected = false;
            server.status_text = "events stream disconnected".to_string();
        }
    });
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    replace_stream(&mut streams, server_id, source);
}

fn replace_stream(
    streams: &mut Signal<Vec<StreamHandle>>,
    server_id: usize,
    source: web_sys::EventSource,
) {
    let mut handles = streams.write();
    if let Some(pos) = handles.iter().position(|s| s.server_id == server_id) {
        handles[pos].source.close();
        handles.remove(pos);
    }
    handles.push(StreamHandle { server_id, source });
}

fn close_stream_for_server(mut streams: Signal<Vec<StreamHandle>>, server_id: usize) {
    let mut handles = streams.write();
    if let Some(pos) = handles.iter().position(|s| s.server_id == server_id) {
        handles[pos].source.close();
        handles.remove(pos);
    }
}

fn apply_chunk_to_server(app: &mut Signal<AppState>, server_id: usize, chunk: StreamChunk) {
    let mut state = app.write();
    let Some(server) = find_server_mut_by_id(&mut state, server_id) else {
        return;
    };
    let kind = chunk.kind.clone();
    if should_ignore_replayed_chunk(server, &kind) {
        return;
    }

    match kind.as_str() {
        "submit" => {
            if !handle_submit_chunk(server, &chunk) {
                return;
            }
        }
        "status" | "heartbeat" => {
            if let Some(message) = chunk.message {
                server.status_text = message;
            }
        }
        "event" => {
            if let Some(event) = chunk.event {
                apply_ui_event(server, event);
            }
        }
        "delta" => {
            if let Some(text) = chunk.text {
                if let Some(turn) = active_response_turn_mut(server) {
                    turn.markdown.push_str(&text);
                }
            }
        }
        "final" => {
            handle_final_chunk(server, chunk);
        }
        "snapshot" => {
            if let Some(snapshot) = chunk.snapshot {
                handle_snapshot_chunk(server, &snapshot, "snapshot synced");
            }
        }
        "error" => {
            handle_error_chunk(
                server,
                chunk.message.unwrap_or_else(|| "stream error".to_string()),
            );
        }
        _ => {}
    }

    if !server.conversation_follow && is_user_visible_update(&kind) {
        server.conversation_unseen = server.conversation_unseen.saturating_add(1);
    }
}

fn should_ignore_replayed_chunk(server: &mut ServerSession, kind: &str) -> bool {
    if !server.suppress_replayed_loop {
        return false;
    }
    match kind {
        "event" | "delta" => true,
        "final" | "error" => {
            server.suppress_replayed_loop = false;
            true
        }
        _ => false,
    }
}

fn handle_submit_chunk(server: &mut ServerSession, chunk: &StreamChunk) -> bool {
    if is_duplicate_replayed_submit(server, chunk.submitted.as_deref()) {
        server.suppress_replayed_loop = true;
        return false;
    }

    server.pending = true;
    if let Some(submitted) = &chunk.submitted {
        append_user_turn_if_new(server, submitted);
    }
    begin_streaming_response(server);
    server.status_text = chunk
        .submitted
        .clone()
        .map(|s| format!("submitted: {}", truncate(&s, 72)))
        .unwrap_or_else(|| "submitted".to_string());
    true
}

fn handle_final_chunk(server: &mut ServerSession, chunk: StreamChunk) {
    server.pending = false;
    server.status_text = chunk.message.unwrap_or_else(|| "done".to_string());
    sync_active_response_agent_roots(server);
    if let Some(turn) = active_response_turn_mut(server) {
        turn.agent_details_collapsed = true;
    }

    if let Some(response) = chunk.response {
        server.model = response.state.model;
        server.session_id = response.state.session.id;
        if let Some(turn) = active_response_turn_mut(server) {
            if turn.markdown.trim().is_empty() && !response.output.trim().is_empty() {
                turn.markdown = response.output;
            }
        }
    }
    server.active_response_turn_id = None;
}

fn handle_snapshot_chunk(server: &mut ServerSession, snapshot: &BackendSnapshot, status: &str) {
    server.model = snapshot.model.clone();
    server.session_id = snapshot.session.id.clone();
    hydrate_turns_from_snapshot(server, snapshot);
    server.connected = true;
    server.status_text = status.to_string();
}

fn handle_error_chunk(server: &mut ServerSession, message: String) {
    server.pending = false;
    server.connected = false;
    server.status_text = message;
    server.active_response_turn_id = None;
    server.suppress_replayed_loop = false;
}

fn apply_ui_event(server: &mut ServerSession, event: UiEvent) {
    let payload = match &event.payload {
        Some(payload) => payload,
        None => {
            server.status_text = format!("{}: {}", event.title, event.detail);
            return;
        }
    };

    let agent = payload
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("agent")
        .to_string();
    let depth = payload
        .get("handoff_depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let phase = payload
        .get("phase")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let event_sequence = payload.get("sequence").and_then(|v| v.as_u64());
    let event_id = payload
        .get("event_id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);

    server.status_text = format!("{}: {}", event.title, event.detail);
    let node_id = ensure_node(server, agent, depth);
    capture_handoff_start(server, &event, node_id, depth);
    let mut response_author: Option<String> = None;

    if let Some(node) = server.nodes.iter_mut().find(|n| n.id == node_id) {
        if event.kind == "model" && phase == "response_received" {
            response_author = Some(node.name.clone());
        }
        node.reasoning_text = Some(truncate(&event.detail, 240));
        node.phase_badge = phase_badge(depth, &event.kind, phase).to_string();
        push_timeline(
            &mut node.timeline,
            &event.kind,
            phase_or_default(phase),
            event_sequence,
            event_id.as_deref(),
            &event.detail,
        );

        if event.kind == "model" && phase == "response_received" {
            let sent = payload
                .get("prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let received = payload
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache = payload
                .get("cached_prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cost = payload
                .get("cost_usd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            node.sent = node.sent.saturating_add(sent);
            node.received = node.received.saturating_add(received);
            node.cache = node.cache.saturating_add(cache);
            node.cost_usd += cost;
            node.context_pct = context_pct(node.sent, &server.model);

            if depth == 0 {
                server.totals.top_agent = node.name.clone();
                server.totals.tokens_sent = node.sent;
                server.totals.tokens_received = node.received;
                server.totals.cached_tokens = node.cache;
                server.totals.context_pct = node.context_pct;
                server.totals.cost_usd = node.cost_usd;
            }
        }

        node.last_event = if phase.is_empty() {
            truncate(&event.detail, 120)
        } else {
            format!("{} - {}", phase, truncate(&event.detail, 96))
        };
    }

    if let Some(author) = response_author {
        if let Some(turn) = active_response_turn_mut(server) {
            turn.author = author;
        }
    }
    sync_active_response_agent_roots(server);
}

fn capture_handoff_start(
    server: &mut ServerSession,
    event: &UiEvent,
    parent_id: usize,
    depth: u32,
) {
    if event.kind != "tool" {
        return;
    }

    let Some(payload) = event.payload.as_ref() else {
        return;
    };

    let phase = payload
        .get("phase")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if phase != "start" {
        return;
    }

    let tool_name = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !tool_name.starts_with("handoff_") {
        return;
    }

    let Some(arguments) = payload.get("arguments") else {
        return;
    };
    let (message, context) = extract_handoff_args(arguments);
    if message.is_empty() {
        return;
    }

    let child_depth = depth.saturating_add(1);
    server
        .pending_handoffs
        .retain(|h| !(h.parent_id == parent_id && h.depth == child_depth));
    server.pending_handoffs.push(PendingHandoff {
        parent_id,
        depth: child_depth,
        message,
        context,
    });
}

fn extract_handoff_args(value: &serde_json::Value) -> (String, Option<String>) {
    if let Some(obj) = value.as_object() {
        let task = obj
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let context = obj
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());
        return (task, context);
    }

    if let Some(raw) = value.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            return extract_handoff_args(&parsed);
        }
        return (raw.to_string(), None);
    }

    (String::new(), None)
}

fn ensure_node(server: &mut ServerSession, agent: String, depth: u32) -> usize {
    while server.lineage.len() <= depth as usize {
        server.lineage.push(None);
    }

    let parent_id = if depth == 0 {
        None
    } else {
        server
            .lineage
            .get(depth as usize - 1)
            .and_then(|entry| *entry)
    };

    if let Some(existing) = server
        .nodes
        .iter()
        .find(|n| n.name == agent && n.depth == depth && n.parent_id == parent_id)
        .map(|n| n.id)
    {
        server.lineage[depth as usize] = Some(existing);
        server.lineage.truncate(depth as usize + 1);
        return existing;
    }

    let id = server.next_node_id;
    server.next_node_id = server.next_node_id.saturating_add(1);
    let (handoff_message, handoff_context) = consume_pending_handoff(server, parent_id, depth);
    server.nodes.push(AgentNode {
        id,
        parent_id,
        name: agent,
        depth,
        phase_badge: "idle".to_string(),
        sent: 0,
        received: 0,
        cache: 0,
        context_pct: 0.0,
        cost_usd: 0.0,
        reasoning_text: None,
        handoff_message,
        handoff_context,
        last_event: "waiting".to_string(),
        timeline: Vec::new(),
    });

    server.lineage[depth as usize] = Some(id);
    server.lineage.truncate(depth as usize + 1);
    id
}

fn build_tree(server: &ServerSession) -> Vec<AgentNodeView> {
    build_children(&server.nodes, None)
}

fn build_children(nodes: &[AgentNode], parent_id: Option<usize>) -> Vec<AgentNodeView> {
    let mut output = Vec::new();
    for node in nodes.iter().filter(|n| n.parent_id == parent_id) {
        output.push(AgentNodeView {
            id: node.id,
            name: node.name.clone(),
            depth: node.depth,
            phase_badge: node.phase_badge.clone(),
            tokens_sent: node.sent,
            tokens_received: node.received,
            cached_tokens: node.cache,
            context_pct: node.context_pct,
            cost_usd: node.cost_usd,
            reasoning_text: node.reasoning_text.clone(),
            handoff_message: node.handoff_message.clone(),
            handoff_context: node.handoff_context.clone(),
            last_event: node.last_event.clone(),
            timeline: node
                .timeline
                .iter()
                .map(|entry| ui::AgentTimelineEntryView {
                    text: entry.text.clone(),
                    kind: entry.kind.clone(),
                    phase: entry.phase.clone(),
                    sequence: entry.sequence,
                    event_id: entry.event_id.clone(),
                })
                .collect(),
            children: build_children(nodes, Some(node.id)),
        });
    }
    output
}

fn consume_pending_handoff(
    server: &mut ServerSession,
    parent_id: Option<usize>,
    depth: u32,
) -> (Option<String>, Option<String>) {
    let Some(parent_id) = parent_id else {
        return (None, None);
    };

    if let Some(idx) = server
        .pending_handoffs
        .iter()
        .position(|h| h.parent_id == parent_id && h.depth == depth)
    {
        let pending = server.pending_handoffs.remove(idx);
        return (Some(pending.message), pending.context);
    }

    (None, None)
}

fn find_server_mut_by_id(state: &mut AppState, id: usize) -> Option<&mut ServerSession> {
    state.servers.iter_mut().find(|s| s.id == id)
}

fn current_server(state: &AppState) -> Option<ServerSession> {
    state
        .servers
        .iter()
        .find(|s| s.id == state.active_tab_id)
        .cloned()
}

fn current_server_mut(state: &mut AppState) -> Option<&mut ServerSession> {
    let id = state.active_tab_id;
    state.servers.iter_mut().find(|s| s.id == id)
}

fn context_pct(prompt_tokens: u64, model: &str) -> f64 {
    let window = model_context_window_tokens(model);
    if window == 0 {
        return 0.0;
    }
    (prompt_tokens as f64 / window as f64) * 100.0
}

fn model_context_window_tokens(model: &str) -> u64 {
    let model = model.to_ascii_lowercase();
    if model.contains("gpt-4.1") || model.contains("gpt-5") {
        return 1_048_576;
    }
    if model.contains("gpt-4o") {
        return 128_000;
    }
    if model.contains("o1") || model.contains("o3") || model.contains("o4") {
        return 200_000;
    }
    if model.contains("claude-3") || model.contains("claude-sonnet-4") {
        return 200_000;
    }
    if model.contains("gemini") {
        return 1_048_576;
    }
    128_000
}

fn phase_badge(depth: u32, kind: &str, phase: &str) -> &'static str {
    match (kind, phase) {
        ("model", "request_started") if depth > 0 => "handoff active",
        ("model", "response_received") if depth > 0 => "handoff done",
        ("model", "request_started") => "active",
        ("model", "response_received") => "done",
        ("tool", "start") => "tool active",
        ("tool", "finish") => "tool done",
        _ => "idle",
    }
}

fn phase_or_default(phase: &str) -> &str {
    if phase.is_empty() {
        "update"
    } else {
        phase
    }
}

fn push_timeline(
    lines: &mut Vec<AgentTimelineEntry>,
    kind: &str,
    phase: &str,
    sequence: Option<u64>,
    event_id: Option<&str>,
    detail: &str,
) {
    let text = truncate(&format!("{kind} | {phase} | {detail}"), 180);
    lines.push(AgentTimelineEntry {
        text,
        kind: kind.to_string(),
        phase: phase.to_string(),
        sequence,
        event_id: event_id.map(ToString::to_string),
    });
    if lines.len() > 40 {
        let drain = lines.len() - 40;
        lines.drain(0..drain);
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let mut out = input.chars().take(max).collect::<String>();
    out.push_str("...");
    out
}

fn begin_streaming_response(server: &mut ServerSession) {
    server.nodes.clear();
    server.lineage.clear();
    server.pending_handoffs.clear();
    server.next_node_id = 0;
    server.totals = FooterTotalsView {
        top_agent: "none".to_string(),
        ..FooterTotalsView::default()
    };

    if let Some(turn) = active_response_turn_mut(server) {
        turn.author = "assistant".to_string();
        turn.role_class = "assistant".to_string();
        turn.markdown.clear();
        turn.agent_roots.clear();
        turn.agent_details_collapsed = false;
        turn.agent_details_enabled = true;
        return;
    }

    if let Some(id) = append_turn(
        server,
        "assistant".to_string(),
        "assistant".to_string(),
        String::new(),
        true,
    ) {
        server.active_response_turn_id = Some(id);
    }
}

fn active_response_turn_mut(server: &mut ServerSession) -> Option<&mut ChatTurn> {
    let active_id = server.active_response_turn_id?;
    let idx = server.turns.iter().position(|turn| turn.id == active_id)?;
    server.turns.get_mut(idx)
}

fn sync_active_response_agent_roots(server: &mut ServerSession) {
    let roots = build_tree(server);
    if let Some(turn) = active_response_turn_mut(server) {
        turn.agent_roots = roots;
        turn.agent_details_enabled = true;
    }
}

fn append_turn(
    server: &mut ServerSession,
    author: String,
    role_class: String,
    markdown: String,
    allow_empty_markdown: bool,
) -> Option<usize> {
    if !allow_empty_markdown && markdown.trim().is_empty() {
        return None;
    }
    let id = server.next_turn_id;
    server.next_turn_id = server.next_turn_id.saturating_add(1);
    server.turns.push(ChatTurn {
        id,
        author,
        role_class,
        markdown,
        agent_roots: Vec::new(),
        agent_details_collapsed: false,
        agent_details_enabled: true,
    });
    Some(id)
}

fn user_turn_markdown(prompt: &str) -> String {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return String::new();
    }
    format!("```text\n{}\n```", prompt)
}

fn append_user_turn_if_new(server: &mut ServerSession, prompt: &str) {
    let markdown = user_turn_markdown(prompt);
    if markdown.trim().is_empty() {
        return;
    }

    let is_duplicate = server
        .turns
        .last()
        .is_some_and(|turn| turn.role_class == "user" && turn.markdown == markdown)
        || server.active_response_turn_id.is_some_and(|active_id| {
            server.turns.last().is_some_and(|turn| turn.id == active_id)
                && has_trailing_user_assistant_pair(server, &markdown)
        });
    if is_duplicate {
        return;
    }

    let _ = append_turn(
        server,
        "you".to_string(),
        "user".to_string(),
        markdown,
        false,
    );
}

fn build_chat_turns(server: &ServerSession) -> Vec<ChatTurnView> {
    server
        .turns
        .iter()
        .map(|turn| ChatTurnView {
            id: turn.id,
            author: turn.author.clone(),
            role_class: turn.role_class.clone(),
            markdown: turn.markdown.clone(),
            agent_roots: turn.agent_roots.clone(),
            agent_details_collapsed: turn.agent_details_collapsed,
            agent_details_enabled: turn.agent_details_enabled,
        })
        .collect::<Vec<_>>()
}

fn apply_model_event_snapshot(server: &mut ServerSession, snapshot: ModelEventDetailsSnapshot) {
    server.model_json_session_id = snapshot.session_id;
    server.model_json_events = snapshot.events;
}

fn pretty_json_value(value: Option<&serde_json::Value>) -> String {
    let Some(value) = value else {
        return "{}".to_string();
    };
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn is_user_visible_update(kind: &str) -> bool {
    matches!(kind, "submit" | "event" | "delta" | "final" | "error")
}

fn is_duplicate_replayed_submit(server: &ServerSession, submitted: Option<&str>) -> bool {
    let Some(submitted) = submitted else {
        return false;
    };
    if server.pending || server.active_response_turn_id.is_some() {
        return false;
    }

    let expected_user = user_turn_markdown(submitted);
    has_trailing_user_assistant_pair(server, &expected_user)
}

fn has_trailing_user_assistant_pair(server: &ServerSession, expected_user_markdown: &str) -> bool {
    if server.turns.len() < 2 {
        return false;
    }

    let last = &server.turns[server.turns.len() - 1];
    let prev = &server.turns[server.turns.len() - 2];
    last.role_class == "assistant"
        && prev.role_class == "user"
        && prev.markdown == expected_user_markdown
}

fn hydrate_turns_from_snapshot(server: &mut ServerSession, snapshot: &BackendSnapshot) {
    if server.pending || server.active_response_turn_id.is_some() || !server.turns.is_empty() {
        return;
    }
    if snapshot.conversation.is_empty() {
        return;
    }

    server.turns.clear();
    server.next_turn_id = 0;
    server.active_response_turn_id = None;
    server.nodes.clear();
    server.lineage.clear();
    server.pending_handoffs.clear();
    server.next_node_id = 0;

    for line in &snapshot.conversation {
        append_snapshot_line(server, line);
    }
}

fn hydrate_agent_details_from_ui_events(server: &mut ServerSession, events: &[UiEvent]) {
    if events.is_empty() {
        return;
    }

    let mut loops: Vec<Vec<UiEvent>> = Vec::new();
    for event in events {
        let Some(payload) = event.payload.as_ref() else {
            continue;
        };
        let depth = payload
            .get("handoff_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let phase = payload
            .get("phase")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let turn = payload.get("turn").and_then(|v| v.as_u64()).unwrap_or(0);
        let is_loop_start =
            event.kind == "model" && depth == 0 && phase == "request_started" && turn == 1;
        if is_loop_start || loops.is_empty() {
            loops.push(Vec::new());
        }
        if let Some(bucket) = loops.last_mut() {
            bucket.push(event.clone());
        }
    }

    if loops.is_empty() {
        return;
    }

    let assistant_indices = server
        .turns
        .iter()
        .enumerate()
        .filter_map(|(idx, turn)| (turn.role_class == "assistant").then_some(idx))
        .collect::<Vec<_>>();
    let assign_count = assistant_indices.len().min(loops.len());
    if assign_count == 0 {
        return;
    }

    for i in 0..assign_count {
        let roots = build_agent_roots_from_events(&server.model, &loops[i]);
        if let Some(turn) = server.turns.get_mut(assistant_indices[i]) {
            turn.agent_roots = roots;
            turn.agent_details_enabled = !turn.agent_roots.is_empty();
            turn.agent_details_collapsed = true;
        }
    }
}

fn build_agent_roots_from_events(model: &str, events: &[UiEvent]) -> Vec<AgentNodeView> {
    let mut temp = new_server_session(0, DEFAULT_SERVER_URL);
    temp.model = model.to_string();
    for event in events {
        apply_ui_event(&mut temp, event.clone());
    }
    build_tree(&temp)
}

fn append_snapshot_line(server: &mut ServerSession, line: &ConversationLine) {
    match line.role.as_str() {
        "you" => {
            let markdown = user_turn_markdown(&line.text);
            let _ = append_turn(
                server,
                "you".to_string(),
                "user".to_string(),
                markdown,
                false,
            );
        }
        "assistant" => {
            let _ = append_turn(
                server,
                "assistant".to_string(),
                "assistant".to_string(),
                line.text.clone(),
                false,
            );
        }
        _ => {}
    }
}

fn current_origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string())
}

fn default_server_urls() -> Vec<String> {
    vec![DEFAULT_SERVER_URL.to_string()]
}

fn unique_urls(urls: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for url in urls {
        if !out.iter().any(|u| u == &url) {
            out.push(url);
        }
    }
    out
}

fn persist_tabs(state: &AppState) {
    let payload = PersistedTabs {
        urls: state
            .servers
            .iter()
            .map(|s| s.client.base_url.clone())
            .collect::<Vec<_>>(),
        active_url: state
            .servers
            .iter()
            .find(|s| s.id == state.active_tab_id)
            .map(|s| s.client.base_url.clone()),
    };

    if let Ok(serialized) = serde_json::to_string(&payload) {
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok())
            .flatten();
        if let Some(storage) = storage {
            let _ = storage.set_item(STORAGE_KEY, &serialized);
        }
    }
}

fn load_persisted_tabs() -> Option<PersistedTabs> {
    let storage = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()?;
    let raw = storage.get_item(STORAGE_KEY).ok().flatten()?;
    serde_json::from_str::<PersistedTabs>(&raw).ok()
}
