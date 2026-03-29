use api::{
    normalize_base_url, BackendSnapshot, ConversationLine, ModelEventDetail,
    ModelEventDetailsSnapshot, ProxyClient, StreamChunk, TranscribeResponse, UiEvent,
};
use dioxus::prelude::*;
use dioxus::web::WebEventExt;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use ui::{
    AgentNodeView, ChatTurnView, Dashboard, FooterTotalsView, PromptImagePreviewView, ServerTabView,
};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

const MAIN_CSS: Asset = asset!("/assets/main.css");
const HLJS_THEME: &str =
    "https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.1/styles/github.min.css";
const HLJS_SCRIPT: &str =
    "https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.11.1/highlight.min.js";
const STORAGE_KEY: &str = "zagent.dioxus.servers.v1";
const MERMAID_JS: &str = "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js";
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
    prompt_images: Vec<PromptImageAttachment>,
    next_prompt_image_id: usize,
    recording: bool,
    transcribing: bool,
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
    group_key: String,
    title: String,
    request_text: String,
    response_text: Option<String>,
    kind: String,
    phase: String,
    sequence: Option<u64>,
    request_event_id: Option<String>,
    response_event_id: Option<String>,
    tool_call_id: Option<String>,
    running: bool,
    show_terminal: bool,
    terminal_segments: Vec<TerminalSegment>,
}

#[derive(Debug, Clone)]
struct TerminalSegment {
    channel: String,
    text: String,
}

#[derive(Debug, Clone)]
struct PromptImageAttachment {
    id: usize,
    name: String,
    preview_url: String,
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

struct RecorderHandle {
    stream: web_sys::MediaStream,
    context: web_sys::AudioContext,
    source: web_sys::MediaStreamAudioSourceNode,
    processor: web_sys::ScriptProcessorNode,
    samples: Rc<RefCell<Vec<f32>>>,
    _on_audio: Closure<dyn FnMut(web_sys::AudioProcessingEvent)>,
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
    let recorder_handle = use_hook(|| Rc::new(RefCell::new(None::<RecorderHandle>)));

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
        Dashboard {
            title: "zAgent Dioxus".to_string(),
            status_text: current
                .as_ref()
                .map(|s| s.status_text.clone())
                .unwrap_or_else(|| "No server selected".to_string()),
            connected: current.as_ref().is_some_and(|s| s.connected),
            pending: current.as_ref().is_some_and(|s| s.pending),
            recording: current.as_ref().is_some_and(|s| s.recording),
            transcribing: current.as_ref().is_some_and(|s| s.transcribing),
            prompt_value: current
                .as_ref()
                .map(|s| s.prompt.clone())
                .unwrap_or_default(),
            prompt_images: current
                .as_ref()
                .map(|s| s.prompt_images.iter().map(|image| PromptImagePreviewView {
                    id: image.id,
                    name: image.name.clone(),
                    data_url: image.preview_url.clone()
                }).collect())
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
            on_copy_prompt: Some(EventHandler::new(move |_| {
                if let Some(server) = current_server(&app()) {
                    copy_to_clipboard(server.prompt);
                }
            })),
            on_paste_prompt: Some(EventHandler::new(move |_| {
                paste_from_clipboard(app);
            })),
            on_prompt_paste: Some(EventHandler::new(move |evt| {
                handle_prompt_paste(app, evt);
            })),
            on_pick_images: Some(EventHandler::new(move |_| {
                trigger_prompt_image_picker();
            })),
            on_remove_prompt_image: move |image_id| {
                if let Some(server) = current_server_mut(&mut app.write()) {
                    if let Some(pos) = server.prompt_images.iter().position(|image| image.id == image_id) {
                        revoke_preview_url(&server.prompt_images[pos].preview_url);
                        server.prompt_images.remove(pos);
                    }
                }
            },
            on_toggle_recording: Some(EventHandler::new(move |_| {
                toggle_recording(app, recorder_handle.clone());
            })),
            on_submit: move |_| {
                let (server_id, client, text) = {
                    let mut state = app.write();
                    let Some(server) = current_server_mut(&mut state) else {
                        return;
                    };
                    let text = compose_prompt_submission(server);
                    if text.is_empty() || server.pending || server.transcribing {
                        return;
                    }
                    server.pending = true;
                    server.status_text = "message accepted".to_string();
                    server.conversation_follow = true;
                    server.conversation_unseen = 0;
                    append_user_turn_if_new(server, &text);
                    begin_streaming_response(server);
                    server.prompt.clear();
                    clear_prompt_images(server);
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
        input {
            id: "prompt-image-picker",
            r#type: "file",
            accept: "image/*",
            multiple: true,
            hidden: true,
            onchange: move |evt| {
                handle_prompt_image_selection(app, evt);
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
    let script = format!(
        "(function () {{ \
            const scriptSrc = {script_src:?}; \
            window.__zagentRenderMermaid = async function () {{ \
                if (!window.mermaid) {{ \
                    let script = document.querySelector('script[data-zagent-mermaid-script]'); \
                    if (!script) {{ \
                        script = document.createElement('script'); \
                        script.src = scriptSrc; \
                        script.async = true; \
                        script.dataset.zagentMermaidScript = 'true'; \
                        script.onload = function () {{ \
                            if (window.__zagentRenderMermaid) {{ \
                                window.__zagentRenderMermaid(); \
                            }} \
                        }}; \
                        script.onerror = function () {{ \
                            document.querySelectorAll('.mermaid-graph:not([data-mermaid-rendered])').forEach((node) => {{ \
                                node.setAttribute('data-mermaid-rendered', 'error'); \
                                node.setAttribute('data-mermaid-error', 'Failed to load Mermaid script'); \
                            }}); \
                        }}; \
                        document.head.appendChild(script); \
                    }} \
                    return; \
                }} \
                if (!window.mermaidInitialized) {{ \
                    window.mermaid.initialize({{ startOnLoad: false, securityLevel: 'loose' }}); \
                    window.mermaidInitialized = true; \
                }} \
                const nodes = document.querySelectorAll('.mermaid-graph:not([data-mermaid-rendered])'); \
                for (const node of nodes) {{ \
                    const source = (node.textContent || '').trim(); \
                    if (!source) {{ \
                        node.setAttribute('data-mermaid-rendered', 'empty'); \
                        continue; \
                    }} \
                    try {{ \
                        const id = 'zagent-mermaid-' + Math.random().toString(36).slice(2); \
                        const rendered = await window.mermaid.render(id, source); \
                        node.innerHTML = rendered.svg; \
                        node.setAttribute('data-mermaid-rendered', 'true'); \
                        if (rendered.bindFunctions) {{ \
                            rendered.bindFunctions(node); \
                        }} \
                    }} catch (err) {{ \
                        console.error('Failed to render mermaid diagram', err); \
                        node.setAttribute('data-mermaid-rendered', 'error'); \
                        node.setAttribute('data-mermaid-error', String(err)); \
                    }} \
                }} \
            }}; \
            window.__zagentRenderMermaid(); \
        }})();",
        script_src = MERMAID_JS,
    );
    let _ = js_sys::eval(&script);
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
        prompt_images: Vec::new(),
        next_prompt_image_id: 0,
        recording: false,
        transcribing: false,
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

fn trigger_prompt_image_picker() {
    let document = web_sys::window().and_then(|w| w.document());
    let Some(input) = document
        .and_then(|d| d.get_element_by_id("prompt-image-picker"))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
    else {
        return;
    };
    input.set_value("");
    input.click();
}

fn handle_prompt_image_selection(mut app: Signal<AppState>, evt: Event<FormData>) {
    let files = evt.files();
    if files.is_empty() {
        return;
    }
    let active_tab_id = app().active_tab_id;
    let mut state = app.write();
    let Some(server) = find_server_mut_by_id(&mut state, active_tab_id) else {
        return;
    };

    for file in files {
        push_prompt_image(
            server,
            &file.name(),
            file.inner().downcast_ref::<web_sys::File>(),
        );
    }
}

fn push_prompt_image(server: &mut ServerSession, name: &str, file: Option<&web_sys::File>) -> bool {
    let Some(file) = file else {
        return false;
    };
    let Ok(preview_url) = web_sys::Url::create_object_url_with_blob(file) else {
        return false;
    };
    let image_id = server.next_prompt_image_id;
    server.next_prompt_image_id = server.next_prompt_image_id.saturating_add(1);
    server.prompt_images.push(PromptImageAttachment {
        id: image_id,
        name: name.to_string(),
        preview_url,
    });
    true
}

fn clear_prompt_images(server: &mut ServerSession) {
    for image in &server.prompt_images {
        revoke_preview_url(&image.preview_url);
    }
    server.prompt_images.clear();
}

fn revoke_preview_url(url: &str) {
    let _ = web_sys::Url::revoke_object_url(url);
}

fn compose_prompt_submission(server: &ServerSession) -> String {
    let mut text = server.prompt.trim().to_string();
    if !server.prompt_images.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("Attached image previews:\n");
        for image in &server.prompt_images {
            text.push_str(&format!("- {}\n", image.name));
        }
    }
    text
}

fn copy_to_clipboard(text: String) {
    spawn(async move {
        let Some(clipboard) = web_sys::window().map(|w| w.navigator().clipboard()) else {
            return;
        };
        let _ = JsFuture::from(clipboard.write_text(&text)).await;
    });
}

fn paste_from_clipboard(app: Signal<AppState>) {
    spawn(async move {
        let Some(clipboard) = web_sys::window().map(|w| w.navigator().clipboard()) else {
            return;
        };
        let Ok(value) = JsFuture::from(clipboard.read_text()).await else {
            return;
        };
        append_prompt_text(app, &value.as_string().unwrap_or_default());
    });
}

fn handle_prompt_paste(mut app: Signal<AppState>, evt: dioxus::events::ClipboardEvent) {
    let Some(web_event) = evt.data().try_as_web_event() else {
        return;
    };
    let Ok(clipboard_event) = web_event.dyn_into::<web_sys::ClipboardEvent>() else {
        return;
    };
    let Some(data) = clipboard_event.clipboard_data() else {
        return;
    };
    let items = data.items();
    let active_tab_id = app().active_tab_id;
    let mut state = app.write();
    let Some(server) = find_server_mut_by_id(&mut state, active_tab_id) else {
        return;
    };

    let mut added_image = false;
    for idx in 0..items.length() {
        let Some(item) = items.get(idx) else {
            continue;
        };
        if !item.kind().eq_ignore_ascii_case("file") {
            continue;
        }
        let mime = item.type_();
        if !mime.starts_with("image/") {
            continue;
        }
        let Ok(Some(file)) = item.get_as_file() else {
            continue;
        };
        let name = if file.name().is_empty() {
            format!("pasted-image-{}.png", server.next_prompt_image_id)
        } else {
            file.name()
        };
        added_image |= push_prompt_image(server, &name, Some(&file));
    }

    if added_image {
        clipboard_event.prevent_default();
    }
}

fn append_prompt_text(mut app: Signal<AppState>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(server) = current_server_mut(&mut app.write()) {
        append_prompt_text_to_server(server, text);
    }
}

fn append_prompt_text_to_server(server: &mut ServerSession, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if server.prompt.trim().is_empty() {
        server.prompt = text.to_string();
    } else {
        server.prompt.push('\n');
        server.prompt.push_str(text);
    }
}

fn toggle_recording(
    mut app: Signal<AppState>,
    recorder_handle: Rc<RefCell<Option<RecorderHandle>>>,
) {
    let active_tab_id = app().active_tab_id;
    let Some(current) = current_server(&app()) else {
        return;
    };
    if current.transcribing {
        return;
    }
    if current.recording {
        if let Some(server) = find_server_mut_by_id(&mut app.write(), active_tab_id) {
            server.recording = false;
            server.transcribing = true;
            server.status_text = "Transcribing voice note...".to_string();
        }
        let client = current.client.clone();
        let handle = recorder_handle.borrow_mut().take();
        spawn(async move {
            let Some(handle) = handle else {
                return;
            };
            let wav = finalize_recording(handle).await;
            let result = client.transcribe_wav(&wav, Some("en")).await;
            if let Some(server) = find_server_mut_by_id(&mut app.write(), active_tab_id) {
                server.recording = false;
                server.transcribing = false;
                match result {
                    Ok(TranscribeResponse { text }) => {
                        append_prompt_text_to_server(server, &text);
                        server.status_text = "Voice transcription ready".to_string();
                    }
                    Err(err) => server.status_text = err,
                }
            }
        });
        return;
    }

    if let Some(server) = find_server_mut_by_id(&mut app.write(), active_tab_id) {
        server.recording = true;
        server.transcribing = false;
        server.status_text = "Requesting microphone...".to_string();
    }

    spawn(async move {
        match start_recording().await {
            Ok(handle) => {
                *recorder_handle.borrow_mut() = Some(handle);
                if let Some(server) = find_server_mut_by_id(&mut app.write(), active_tab_id) {
                    server.recording = true;
                    server.transcribing = false;
                    server.status_text = "Recording voice note...".to_string();
                }
            }
            Err(err) => {
                if let Some(server) = find_server_mut_by_id(&mut app.write(), active_tab_id) {
                    server.recording = false;
                    server.transcribing = false;
                    server.status_text = err;
                }
            }
        }
    });
}

async fn start_recording() -> Result<RecorderHandle, String> {
    let navigator = web_sys::window()
        .map(|w| w.navigator())
        .ok_or_else(|| "window unavailable".to_string())?;
    let media_devices = navigator
        .media_devices()
        .map_err(|_| "media devices unavailable".to_string())?;
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&wasm_bindgen::JsValue::TRUE);
    let stream = JsFuture::from(
        media_devices
            .get_user_media_with_constraints(&constraints)
            .map_err(|_| "microphone access failed".to_string())?,
    )
    .await
    .map_err(|_| "microphone access denied".to_string())?
    .dyn_into::<web_sys::MediaStream>()
    .map_err(|_| "invalid media stream".to_string())?;

    let context = web_sys::AudioContext::new().map_err(|_| "audio context failed".to_string())?;
    let source = context
        .create_media_stream_source(&stream)
        .map_err(|_| "audio source failed".to_string())?;
    let processor = context
        .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
            4096, 1, 1,
        )
        .map_err(|_| "audio processor failed".to_string())?;
    let samples = Rc::new(RefCell::new(Vec::<f32>::new()));
    let samples_for_cb = samples.clone();
    let on_audio = Closure::wrap(Box::new(move |event: web_sys::AudioProcessingEvent| {
        let Ok(buffer) = event.input_buffer() else {
            return;
        };
        let Ok(channel) = buffer.get_channel_data(0) else {
            return;
        };
        samples_for_cb.borrow_mut().extend(channel);
    }) as Box<dyn FnMut(_)>);

    processor.set_onaudioprocess(Some(on_audio.as_ref().unchecked_ref()));
    source
        .connect_with_audio_node(&processor)
        .map_err(|_| "audio graph connect failed".to_string())?;
    processor
        .connect_with_audio_node(&context.destination())
        .map_err(|_| "audio graph output failed".to_string())?;

    Ok(RecorderHandle {
        stream,
        context,
        source,
        processor,
        samples,
        _on_audio: on_audio,
    })
}

async fn finalize_recording(handle: RecorderHandle) -> Vec<u8> {
    let _ = handle.source.disconnect();
    let _ = handle.processor.disconnect();
    handle.processor.set_onaudioprocess(None);
    for idx in 0..handle.stream.get_audio_tracks().length() {
        if let Ok(track) = handle
            .stream
            .get_audio_tracks()
            .get(idx)
            .dyn_into::<web_sys::MediaStreamTrack>()
        {
            track.stop();
        }
    }
    let sample_rate = handle.context.sample_rate().round() as u32;
    if let Ok(promise) = handle.context.close() {
        let _ = JsFuture::from(promise).await;
    }
    encode_wav(&handle.samples.borrow(), sample_rate)
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * i16::MAX as f32) as i16;
        pcm.extend_from_slice(&value.to_le_bytes());
    }

    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
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

    if event.kind == "tool_stream" {
        let tool_call_id = payload
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let channel = payload
            .get("channel")
            .and_then(|v| v.as_str())
            .unwrap_or("stdout");
        let text = payload
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !tool_call_id.is_empty() && !text.is_empty() {
            if let Some(entry) = find_timeline_entry_mut(&mut server.nodes, tool_call_id) {
                append_terminal_segment(entry, channel, text);
                server.status_text = format!("shell {} streaming", channel);
                sync_active_response_agent_roots(server);
            }
        }
        return;
    }

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
    let turn = payload.get("turn").and_then(|v| v.as_u64()).unwrap_or(0);
    let event_sequence = payload.get("sequence").and_then(|v| v.as_u64());
    let event_id = payload
        .get("event_id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let tool_call_id = payload
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let tool_name = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let arguments = payload.get("arguments");
    let result_text = payload.get("result").and_then(|v| v.as_str());

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
        if event.kind == "tool" {
            upsert_tool_timeline(
                &mut node.timeline,
                tool_call_id.as_deref(),
                tool_name,
                phase_or_default(phase),
                event_sequence,
                event_id.as_deref(),
                arguments,
                result_text,
                &event.detail,
            );
        } else if event.kind == "model" {
            upsert_model_timeline(
                &mut node.timeline,
                turn,
                phase_or_default(phase),
                event_sequence,
                event_id.as_deref(),
                &event.detail,
            );
        } else {
            push_timeline(
                &mut node.timeline,
                format!("{}-{}", event.kind, event_sequence.unwrap_or(0)),
                &event.kind,
                &event.detail,
                None,
                phase_or_default(phase),
                event_sequence,
                event_id.as_deref(),
                None,
                None,
                None,
                false,
                false,
                &event.detail,
            );
        }

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
                    title: entry.title.clone(),
                    request_text: entry.request_text.clone(),
                    response_text: entry.response_text.clone(),
                    kind: entry.kind.clone(),
                    phase: entry.phase.clone(),
                    sequence: entry.sequence,
                    request_event_id: entry.request_event_id.clone(),
                    response_event_id: entry.response_event_id.clone(),
                    tool_call_id: entry.tool_call_id.clone(),
                    running: entry.running,
                    show_terminal: entry.show_terminal,
                    terminal_segments: entry
                        .terminal_segments
                        .iter()
                        .map(|segment| ui::TerminalSegmentView {
                            channel: segment.channel.clone(),
                            text: segment.text.clone(),
                        })
                        .collect(),
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
    group_key: String,
    kind: &str,
    title: &str,
    request_text: Option<String>,
    phase: &str,
    sequence: Option<u64>,
    request_event_id: Option<&str>,
    response_event_id: Option<&str>,
    tool_call_id: Option<&str>,
    response_text: Option<String>,
    running: bool,
    show_terminal: bool,
    fallback_text: &str,
) {
    lines.push(AgentTimelineEntry {
        group_key,
        title: title.to_string(),
        request_text: request_text.unwrap_or_else(|| truncate(fallback_text, 180)),
        response_text,
        kind: kind.to_string(),
        phase: phase.to_string(),
        sequence,
        request_event_id: request_event_id.map(ToString::to_string),
        response_event_id: response_event_id.map(ToString::to_string),
        tool_call_id: tool_call_id.map(ToString::to_string),
        running,
        show_terminal,
        terminal_segments: Vec::new(),
    });
    trim_timeline(lines);
}

fn upsert_model_timeline(
    lines: &mut Vec<AgentTimelineEntry>,
    turn: u64,
    phase: &str,
    sequence: Option<u64>,
    event_id: Option<&str>,
    detail: &str,
) {
    let key = format!("model-turn-{turn}");
    let title = format!("Model call turn {turn}");
    if let Some(entry) = lines.iter_mut().rev().find(|entry| entry.group_key == key) {
        entry.phase = phase.to_string();
        entry.sequence = sequence;
        if phase == "request_started" {
            entry.request_text = truncate(detail, 220);
            entry.request_event_id = event_id.map(ToString::to_string);
            entry.running = true;
        } else {
            entry.response_text = Some(truncate(detail, 220));
            entry.response_event_id = event_id.map(ToString::to_string);
            entry.running = false;
        }
        return;
    }

    let is_request = phase == "request_started";
    push_timeline(
        lines,
        key,
        "model",
        &title,
        Some(truncate(detail, 220)),
        phase,
        sequence,
        if is_request { event_id } else { None },
        if is_request { None } else { event_id },
        None,
        if is_request {
            None
        } else {
            Some(truncate(detail, 220))
        },
        is_request,
        false,
        detail,
    );
}

fn upsert_tool_timeline(
    lines: &mut Vec<AgentTimelineEntry>,
    tool_call_id: Option<&str>,
    tool_name: &str,
    phase: &str,
    sequence: Option<u64>,
    event_id: Option<&str>,
    arguments: Option<&serde_json::Value>,
    result_text: Option<&str>,
    detail: &str,
) {
    let Some(tool_call_id) = tool_call_id.filter(|id| !id.trim().is_empty()) else {
        push_timeline(
            lines,
            format!("tool-{}", sequence.unwrap_or(0)),
            "tool",
            &format!("Tool call {tool_name}"),
            Some(truncate(detail, 220)),
            phase,
            sequence,
            event_id,
            None,
            None,
            None,
            phase == "start",
            false,
            detail,
        );
        return;
    };

    let request_summary = tool_request_summary(tool_name, arguments);
    let response_summary = tool_response_summary(tool_name, detail, result_text);
    let running = phase == "start";
    let show_terminal = tool_name == "shell_exec";

    if let Some(entry) = lines
        .iter_mut()
        .rev()
        .find(|entry| entry.group_key == format!("tool-{tool_call_id}"))
    {
        entry.phase = phase.to_string();
        entry.sequence = sequence;
        entry.request_text = request_summary.clone();
        entry.running = running;
        entry.show_terminal = show_terminal;
        if phase == "start" {
            entry.request_event_id = event_id.map(ToString::to_string);
        } else {
            entry.response_event_id = event_id.map(ToString::to_string);
            entry.response_text = response_summary.clone();
        }
        if show_terminal && phase == "start" && entry.terminal_segments.is_empty() {
            if let Some(command) = extract_shell_command(arguments) {
                entry.terminal_segments.push(TerminalSegment {
                    channel: "system".to_string(),
                    text: format!("$ {command}\n"),
                });
            }
        }
        if show_terminal && phase == "finish" && result_text.is_some() {
            let command =
                shell_command_from_entry(entry).or_else(|| extract_shell_command(arguments));
            entry.terminal_segments =
                build_shell_terminal_segments(command.as_deref(), result_text);
        }
        return;
    }

    let mut terminal_segments = Vec::new();
    if show_terminal && phase == "start" {
        if let Some(command) = extract_shell_command(arguments) {
            terminal_segments.push(TerminalSegment {
                channel: "system".to_string(),
                text: format!("$ {command}\n"),
            });
        }
    }
    if show_terminal && phase == "finish" && result_text.is_some() {
        terminal_segments =
            build_shell_terminal_segments(extract_shell_command(arguments).as_deref(), result_text);
    }

    lines.push(AgentTimelineEntry {
        group_key: format!("tool-{tool_call_id}"),
        title: format!("Tool call {tool_name}"),
        request_text: request_summary,
        response_text: if phase == "finish" {
            response_summary
        } else {
            None
        },
        kind: "tool".to_string(),
        phase: phase.to_string(),
        sequence,
        request_event_id: if phase == "start" {
            event_id.map(ToString::to_string)
        } else {
            None
        },
        response_event_id: if phase == "finish" {
            event_id.map(ToString::to_string)
        } else {
            None
        },
        tool_call_id: Some(tool_call_id.to_string()),
        running,
        show_terminal,
        terminal_segments,
    });
    trim_timeline(lines);
}

fn trim_timeline(lines: &mut Vec<AgentTimelineEntry>) {
    if lines.len() > 40 {
        let drain = lines.len() - 40;
        lines.drain(0..drain);
    }
}

fn find_timeline_entry_mut<'a>(
    nodes: &'a mut [AgentNode],
    tool_call_id: &str,
) -> Option<&'a mut AgentTimelineEntry> {
    for node in nodes {
        if let Some(entry) = node
            .timeline
            .iter_mut()
            .rev()
            .find(|entry| entry.tool_call_id.as_deref() == Some(tool_call_id))
        {
            return Some(entry);
        }
    }
    None
}

fn append_terminal_segment(entry: &mut AgentTimelineEntry, channel: &str, text: &str) {
    let text = strip_ansi(text);
    if text.is_empty() {
        return;
    }

    if let Some(last) = entry.terminal_segments.last_mut() {
        if last.channel == channel {
            last.text.push_str(&text);
            return;
        }
    }

    entry.terminal_segments.push(TerminalSegment {
        channel: channel.to_string(),
        text,
    });
}

fn extract_shell_command(arguments: Option<&serde_json::Value>) -> Option<String> {
    let arguments = arguments?;
    if let Some(raw) = arguments.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            return extract_shell_command(Some(&parsed));
        }
        return None;
    }

    arguments
        .get("command")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn shell_command_from_entry(entry: &AgentTimelineEntry) -> Option<String> {
    let first = entry.terminal_segments.first()?;
    if first.channel != "system" {
        return None;
    }
    first
        .text
        .strip_prefix("$ ")
        .map(|value| value.trim_end_matches('\n').to_string())
}

fn build_shell_terminal_segments(
    command: Option<&str>,
    result_text: Option<&str>,
) -> Vec<TerminalSegment> {
    let mut segments = Vec::new();
    if let Some(command) = command.filter(|value| !value.trim().is_empty()) {
        segments.push(TerminalSegment {
            channel: "system".to_string(),
            text: format!("$ {command}\n"),
        });
    }

    let Some(result_text) = result_text else {
        return segments;
    };

    let mut current_channel = "system".to_string();
    for line in strip_ansi(result_text).lines() {
        let next_channel = match line {
            "--- stdout ---" => Some("stdout"),
            "--- stderr ---" => Some("stderr"),
            _ => None,
        };
        if let Some(channel) = next_channel {
            current_channel = channel.to_string();
            continue;
        }
        push_terminal_line(&mut segments, &current_channel, line);
    }

    segments
}

fn tool_request_summary(tool_name: &str, arguments: Option<&serde_json::Value>) -> String {
    if tool_name == "shell_exec" {
        return extract_shell_command(arguments)
            .map(|command| truncate(&format!("command: {command}"), 220))
            .unwrap_or_else(|| "command: (unknown)".to_string());
    }

    let argument_text = arguments
        .map(format_tool_arguments)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "(no arguments)".to_string());
    truncate(&argument_text, 220)
}

fn tool_response_summary(
    tool_name: &str,
    detail: &str,
    result_text: Option<&str>,
) -> Option<String> {
    if tool_name == "shell_exec" {
        return Some(truncate(detail, 220));
    }
    result_text
        .map(|result| truncate(&strip_ansi(result), 220))
        .or_else(|| Some(truncate(detail, 220)))
}

fn format_tool_arguments(arguments: &serde_json::Value) -> String {
    if let Some(raw) = arguments.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
            return format_tool_arguments(&parsed);
        }
        return raw.to_string();
    }

    serde_json::to_string(arguments).unwrap_or_else(|_| arguments.to_string())
}

fn push_terminal_line(segments: &mut Vec<TerminalSegment>, channel: &str, line: &str) {
    let mut text = line.to_string();
    text.push('\n');
    if let Some(last) = segments.last_mut() {
        if last.channel == channel {
            last.text.push_str(&text);
            return;
        }
    }
    segments.push(TerminalSegment {
        channel: channel.to_string(),
        text,
    });
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            while let Some(next) = chars.next() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
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
            turn.agent_details_collapsed = false;
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
