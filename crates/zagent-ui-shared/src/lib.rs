use chrono::Local;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use serde::{Deserialize, Serialize};
use tui_markdown::Options as MarkdownOptions;
use tui_markdown::{StyleSheet, from_str_with_options};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: String,
    pub name: String,
    pub model: String,
    pub message_count: u32,
    #[serde(default)]
    pub total_prompt_tokens: u64,
    #[serde(default)]
    pub total_completion_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionListItem {
    pub id: String,
    pub name: String,
    pub model: String,
    #[serde(default)]
    pub message_count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SessionDialogState {
    pub visible: bool,
    pub loading: bool,
    pub filter: String,
    pub selected: usize,
    pub current_session_id: String,
    pub items: Vec<SessionListItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub last_prompt_tokens: u64,
    #[serde(default)]
    pub cached_prompt_tokens: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub credits_remaining: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendSnapshot {
    pub runtime: String,
    #[serde(default)]
    pub provider: String,
    pub model: String,
    pub working_dir: String,
    pub session: SessionSnapshot,
    #[serde(default)]
    pub conversation: Vec<ConversationLine>,
    #[serde(default)]
    pub tool_results: Vec<ToolResultLine>,
    #[serde(default)]
    pub usage: UsageSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationLine {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolResultLine {
    pub tool_name: String,
    pub result: String,
    pub success: bool,
    pub latency_ms: u64,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendMessageResponse {
    pub kind: String,
    pub output: String,
    pub turns: Option<u32>,
    pub tool_calls: Option<u32>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub events: Vec<UiEvent>,
    pub state: BackendSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiEvent {
    pub kind: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub enabled: bool,
    pub state: String,
    #[serde(default)]
    pub tool_count: Option<usize>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpStatusSnapshot {
    pub runtime: String,
    #[serde(default)]
    pub servers: Vec<McpServerStatus>,
}

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub role: String,
    pub text: String,
    pub tool_result: Option<ToolResultLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    #[default]
    Conversation,
    Activity,
}

impl PanelFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Conversation => Self::Activity,
            Self::Activity => Self::Conversation,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Activity => "Feedback",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UiState {
    pub input: String,
    pub input_cursor: usize,
    pub live_output: String,
    pub lines: Vec<ChatLine>,
    pub activity: Vec<String>,
    pub tool_results: Vec<ToolResultLine>,
    pub snapshot: BackendSnapshot,
    pub pending: bool,
    pub show_tool_result_details: bool,
    pub conversation_follow: bool,
    pub conversation_unseen: u32,
    pub conversation_needs_autoscroll: bool,
    pub focus: PanelFocus,
    pub conversation_scroll: u16,
    pub activity_scroll: u16,
    pub stream_estimated_chars: u64,
    pub stream_estimated_tokens_applied: u64,
    pub stream_received_usage_event: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            input_cursor: 0,
            live_output: String::new(),
            lines: Vec::new(),
            activity: vec![stamp("Ready")],
            tool_results: Vec::new(),
            snapshot: BackendSnapshot::default(),
            pending: false,
            show_tool_result_details: false,
            conversation_follow: true,
            conversation_unseen: 0,
            conversation_needs_autoscroll: false,
            focus: PanelFocus::Conversation,
            conversation_scroll: 0,
            activity_scroll: 0,
            stream_estimated_chars: 0,
            stream_estimated_tokens_applied: 0,
            stream_received_usage_event: false,
        }
    }
}

pub fn stamp(msg: &str) -> String {
    format!("[{}] {msg}", Local::now().format("%H:%M:%S"))
}

pub fn prompt_text(input: &str, cursor: usize, show_caret: bool) -> String {
    if !show_caret {
        return format!("> {input}");
    }
    let cursor = cursor.min(input_char_len(input));
    let split = char_to_byte_index(input, cursor);
    let (left, right) = input.split_at(split);
    format!("> {left}|{right}")
}

pub fn input_insert_char(state: &mut UiState, c: char) {
    input_insert_text(state, &c.to_string());
}

pub fn input_insert_text(state: &mut UiState, text: &str) {
    let cursor = state.input_cursor.min(input_char_len(&state.input));
    let split = char_to_byte_index(&state.input, cursor);
    state.input.insert_str(split, text);
    state.input_cursor = cursor + text.chars().count();
}

pub fn input_backspace(state: &mut UiState) {
    let len = input_char_len(&state.input);
    let cursor = state.input_cursor.min(len);
    if cursor == 0 {
        return;
    }
    let start = char_to_byte_index(&state.input, cursor - 1);
    let end = char_to_byte_index(&state.input, cursor);
    state.input.replace_range(start..end, "");
    state.input_cursor = cursor - 1;
}

pub fn input_delete(state: &mut UiState) {
    let len = input_char_len(&state.input);
    let cursor = state.input_cursor.min(len);
    if cursor >= len {
        return;
    }
    let start = char_to_byte_index(&state.input, cursor);
    let end = char_to_byte_index(&state.input, cursor + 1);
    state.input.replace_range(start..end, "");
    state.input_cursor = cursor;
}

pub fn input_move_left(state: &mut UiState) {
    state.input_cursor = state.input_cursor.saturating_sub(1);
}

pub fn input_move_right(state: &mut UiState) {
    let len = input_char_len(&state.input);
    state.input_cursor = (state.input_cursor + 1).min(len);
}

pub fn input_move_home(state: &mut UiState) {
    state.input_cursor = 0;
}

pub fn input_move_end(state: &mut UiState) {
    state.input_cursor = input_char_len(&state.input);
}

pub fn input_clear(state: &mut UiState) {
    state.input.clear();
    state.input_cursor = 0;
}

fn input_char_len(input: &str) -> usize {
    input.chars().count()
}

fn char_to_byte_index(input: &str, char_idx: usize) -> usize {
    if char_idx == 0 {
        return 0;
    }
    input
        .char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(input.len())
}

pub fn conversation_text(lines: &[ChatLine]) -> String {
    let mut out = String::new();
    for line in lines {
        if line.role == "you" {
            out.push_str("──────────────────────── next prompt ────────────────────────\n");
        }
        if line.role == "tool_result" {
            if let Some(tool) = &line.tool_result {
                out.push_str(&format!(
                    "[tool_result] {} {} ({}ms)\n\n",
                    if tool.success { "ok" } else { "fail" },
                    tool.tool_name,
                    tool.latency_ms
                ));
            }
            continue;
        }
        out.push_str(&format!("[{}] {}\n\n", line.role, line.text));
    }
    out
}

pub fn status_line(snapshot: &BackendSnapshot) -> String {
    format!(
        "session={} | provider={} | model={} | wd={}",
        snapshot.session.name, snapshot.provider, snapshot.model, snapshot.working_dir
    )
}

pub fn usage_metrics_line(snapshot: &BackendSnapshot) -> String {
    let credits = snapshot
        .usage
        .credits_remaining
        .map(|v| format!("{v:.4}"))
        .unwrap_or_else(|| "n/a".to_string());
    let ctx_window = model_context_window_tokens(&snapshot.model);
    let context_pct = if ctx_window > 0 {
        (snapshot.usage.last_prompt_tokens as f64 / ctx_window as f64) * 100.0
    } else {
        0.0
    };
    format!(
        "usage total={} prompt={} completion={} context={:.1}% cached={} cost=${:.6} credits={} calls={} turns={} reqs={}",
        snapshot.usage.total_tokens,
        snapshot.session.total_prompt_tokens,
        snapshot.session.total_completion_tokens,
        context_pct,
        snapshot.usage.cached_prompt_tokens,
        snapshot.usage.cost_usd,
        credits,
        snapshot.usage.tool_calls,
        snapshot.usage.turns,
        snapshot.usage.requests
    )
}

pub fn help_dialog_text() -> &'static str {
    "zAgent Help

Commands:
  /help                 Open this help dialog
  /model                Open model selector dialog
  /session              Open session selector dialog
  /mcp                  Open MCP servers status dialog
  /runtime [native|wasi]
  /quit or /q           Exit UI

Input:
  Enter                 Send input
  Ctrl+Enter            Insert newline
  Ctrl+T                Expand/collapse inline tool results
  Ctrl+Shift+C / Ctrl+Shift+V
                        Copy/paste input

Navigation:
  Tab                   Switch focused panel
  Up/Down/PageUp/PageDown/Home/End
                        Scroll focused panel
  Esc / Enter           Close open dialog"
}

pub fn filtered_session_indices(state: &SessionDialogState) -> Vec<usize> {
    let needle = state.filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return (0..state.items.len()).collect();
    }
    state
        .items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            if item.name.to_ascii_lowercase().contains(&needle) {
                Some(idx)
            } else {
                None
            }
        })
        .collect()
}

pub fn session_dialog_entries_len(state: &SessionDialogState) -> usize {
    1 + filtered_session_indices(state).len()
}

pub fn selected_session_index(state: &SessionDialogState) -> Option<usize> {
    if state.selected == 0 {
        return None;
    }
    let filtered = filtered_session_indices(state);
    filtered.get(state.selected - 1).copied()
}

pub fn session_dialog_text(state: &SessionDialogState) -> String {
    if state.loading {
        return "Loading sessions...".to_string();
    }
    let filtered = filtered_session_indices(state);
    let selected = state
        .selected
        .min(session_dialog_entries_len(state).saturating_sub(1));
    let mut out = String::new();
    out.push_str(&format!(
        "Filter: {}\nCurrent session: {}\n\n",
        state.filter, state.current_session_id
    ));

    let create_label = if state.filter.trim().is_empty() {
        "[Create new session]"
    } else {
        "[Create new session with this name]"
    };
    let create_marker = if selected == 0 { ">" } else { " " };
    out.push_str(&format!("{create_marker} {create_label}\n"));

    if filtered.is_empty() {
        out.push_str("  (No saved sessions match filter)\n");
    } else {
        for (offset, idx) in filtered.iter().copied().enumerate() {
            let item = &state.items[idx];
            let row = offset + 1;
            let marker = if row == selected { ">" } else { " " };
            let current = if item.id == state.current_session_id {
                " current"
            } else {
                ""
            };
            out.push_str(&format!(
                "{marker} {} ({}) model={} msgs={}{}\n",
                item.name, item.id, item.model, item.message_count, current
            ));
        }
    }
    out.push_str("\n(Enter to select, Esc to close)");
    out
}

fn model_context_window_tokens(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    if m.contains("gpt-5") || m.contains("gpt-4.1") || m.contains("o3") || m.contains("o4") {
        return 200_000;
    }
    if m.contains("claude-3") {
        return 200_000;
    }
    if m.contains("gemini-1.5") || m.contains("gemini-2") {
        return 1_000_000;
    }
    if m.contains("llama-3") || m.contains("qwen") {
        return 128_000;
    }
    128_000
}

pub fn activity_text(items: &[String]) -> String {
    items.join("\n")
}

pub fn content_max_scroll(text: &str, viewport_height: u16) -> u16 {
    if viewport_height == 0 {
        return 0;
    }
    let line_count = text.lines().count().max(1) as u16;
    line_count.saturating_sub(viewport_height)
}

pub fn wrapped_line_count(text: &str, wrap_width: u16) -> usize {
    let width = wrap_width.max(1) as usize;
    let mut total = 0usize;
    for line in text.lines() {
        let chars = line.chars().count().max(1);
        total += chars.div_ceil(width);
    }
    total.max(1)
}

pub fn content_max_scroll_wrapped(text: &str, viewport_height: u16, wrap_width: u16) -> u16 {
    if viewport_height == 0 {
        return 0;
    }
    let line_count = wrapped_line_count(text, wrap_width) as u16;
    line_count.saturating_sub(viewport_height)
}

pub fn register_submit(state: &mut UiState, submitted: &str) {
    state.lines.push(ChatLine {
        role: "you".to_string(),
        text: submitted.to_string(),
        tool_result: None,
    });
    note_conversation_update(state);
    state.stream_estimated_chars = 0;
    state.stream_estimated_tokens_applied = 0;
    state.stream_received_usage_event = false;
    state.snapshot.usage.requests = state.snapshot.usage.requests.saturating_add(1);
    state.activity.push(stamp("Sending prompt to agent..."));
    trim_log(&mut state.activity, 200);
}

pub fn register_stream_delta(state: &mut UiState, delta: &str) {
    if state.stream_received_usage_event {
        note_conversation_update(state);
        return;
    }
    state.stream_estimated_chars += delta.chars().count() as u64;
    let estimated_completion_tokens = state.stream_estimated_chars / 4;
    if estimated_completion_tokens > state.stream_estimated_tokens_applied {
        let add = estimated_completion_tokens - state.stream_estimated_tokens_applied;
        state.stream_estimated_tokens_applied = estimated_completion_tokens;
        state.snapshot.session.total_completion_tokens = state
            .snapshot
            .session
            .total_completion_tokens
            .saturating_add(add);
        state.snapshot.usage.total_tokens = state.snapshot.usage.total_tokens.saturating_add(add);
    }
    note_conversation_update(state);
}

pub fn register_response(state: &mut UiState, resp: BackendMessageResponse) {
    state.stream_estimated_chars = 0;
    state.stream_estimated_tokens_applied = 0;
    state.stream_received_usage_event = false;
    let events = resp.events.clone();
    state.snapshot = resp.state;
    sync_conversation_from_snapshot(state);
    let role = if resp.kind == "assistant" {
        "assistant"
    } else {
        "system"
    };

    if resp.kind != "assistant" && !resp.output.trim().is_empty() {
        state.lines.push(ChatLine {
            role: role.to_string(),
            text: resp.output.clone(),
            tool_result: None,
        });
    }

    let mut meta = String::from("Received response");
    if let Some(turns) = resp.turns {
        meta.push_str(&format!(" | turns={turns}"));
    }
    if let Some(tool_calls) = resp.tool_calls {
        meta.push_str(&format!(" | tools={tool_calls}"));
    }
    if let (Some(p), Some(c)) = (resp.prompt_tokens, resp.completion_tokens) {
        meta.push_str(&format!(" | tokens={}↑ {}↓", p, c));
    }
    state.activity.push(stamp(&meta));

    for event in events {
        if event.kind == "model" {
            let agent = event
                .payload
                .as_ref()
                .and_then(|p| p.get("agent"))
                .and_then(|v| v.as_str())
                .unwrap_or("root");
            let handoff_depth = event
                .payload
                .as_ref()
                .and_then(|p| p.get("handoff_depth"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let phase = event
                .payload
                .as_ref()
                .and_then(|p| p.get("phase"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let text = if phase == "request_started" {
                let turn = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("turn"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let provider = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("provider"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let model = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("model"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let msg_count = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("message_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let tool_count = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("tool_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                format!(
                    "agent={agent} depth={handoff_depth} request started | turn={turn} provider={provider} model={model} msgs={msg_count} tools={tool_count}"
                )
            } else {
                let turn = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("turn"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let latency = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("latency_ms"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let finish = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("finish_reason"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let prompt = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("prompt_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let completion = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("completion_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let total = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("total_tokens"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                format!(
                    "agent={agent} depth={handoff_depth} response received | turn={turn} latency={latency}ms finish={finish} tokens={prompt}↑ {completion}↓ total={total}"
                )
            };
            state.lines.push(ChatLine {
                role: "model_event".to_string(),
                text,
                tool_result: None,
            });
            note_conversation_update(state);
        }

        if event.kind == "tool" {
            let agent = event
                .payload
                .as_ref()
                .and_then(|p| p.get("agent"))
                .and_then(|v| v.as_str())
                .unwrap_or("root");
            let handoff_depth = event
                .payload
                .as_ref()
                .and_then(|p| p.get("handoff_depth"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tool_name = event
                .payload
                .as_ref()
                .and_then(|p| p.get("tool_name"))
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| event.title.strip_prefix("tool: ").unwrap_or(&event.title))
                .to_string();
            let phase = event
                .payload
                .as_ref()
                .and_then(|p| p.get("phase"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if phase == "start" {
                let arguments = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                state.lines.push(ChatLine {
                    role: "tool_call".to_string(),
                    text: format!(
                        "agent={agent} depth={handoff_depth}\n{}",
                        format_tool_call_line(&tool_name, arguments)
                    ),
                    tool_result: None,
                });
                note_conversation_update(state);
            } else if phase == "finish" {
                let result = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("result"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let success = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("success"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let latency_ms = event
                    .payload
                    .as_ref()
                    .and_then(|p| p.get("latency_ms"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                state.tool_results.push(ToolResultLine {
                    tool_name: format!("{} [{} d={}]", tool_name, agent, handoff_depth),
                    result,
                    success,
                    latency_ms,
                    created_at: Local::now().to_rfc3339(),
                });
                state.lines.push(ChatLine {
                    role: "tool_result".to_string(),
                    text: String::new(),
                    tool_result: state.tool_results.last().cloned(),
                });
                note_conversation_update(state);
                trim_log(&mut state.tool_results, 200);
            }
        }

        if event.kind == "change"
            && event.title == "file_edit"
            && let Some(payload) = &event.payload
        {
            let path = payload
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(&event.detail);
            let diff = payload
                .get("diff")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let lang = infer_language(path);
            state.lines.push(ChatLine {
                role: "file_diff".to_string(),
                text: format!("path:{path}\nlang:{lang}\n{diff}"),
                tool_result: None,
            });
            note_conversation_update(state);
        }
        state
            .activity
            .push(stamp(&format!("{}: {}", event.title, event.detail)));
    }

    trim_log(&mut state.activity, 200);
}

pub fn register_error(state: &mut UiState, error: &str) {
    state.stream_estimated_chars = 0;
    state.stream_estimated_tokens_applied = 0;
    state.stream_received_usage_event = false;
    state.lines.push(ChatLine {
        role: "error".to_string(),
        text: error.to_string(),
        tool_result: None,
    });
    note_conversation_update(state);
    state.activity.push(stamp(&format!("Error: {error}")));
    trim_log(&mut state.activity, 200);
}

pub fn set_feedback(state: &mut UiState, msg: &str) {
    state.activity.push(stamp(msg));
    trim_log(&mut state.activity, 200);
}

pub fn register_stream_event(state: &mut UiState, event: &UiEvent) {
    if event.kind == "model" {
        let agent = event
            .payload
            .as_ref()
            .and_then(|p| p.get("agent"))
            .and_then(|v| v.as_str())
            .unwrap_or("root");
        let handoff_depth = event
            .payload
            .as_ref()
            .and_then(|p| p.get("handoff_depth"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let is_root_agent = handoff_depth == 0;
        let phase = event
            .payload
            .as_ref()
            .and_then(|p| p.get("phase"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let text = if phase == "request_started" {
            let turn = event
                .payload
                .as_ref()
                .and_then(|p| p.get("turn"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let provider = event
                .payload
                .as_ref()
                .and_then(|p| p.get("provider"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let model = event
                .payload
                .as_ref()
                .and_then(|p| p.get("model"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let msg_count = event
                .payload
                .as_ref()
                .and_then(|p| p.get("message_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tool_count = event
                .payload
                .as_ref()
                .and_then(|p| p.get("tool_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if is_root_agent {
                state.snapshot.usage.turns = state.snapshot.usage.turns.max(turn);
            }
            format!(
                "agent={agent} depth={handoff_depth} request started | turn={turn} provider={provider} model={model} msgs={msg_count} tools={tool_count}"
            )
        } else {
            let turn = event
                .payload
                .as_ref()
                .and_then(|p| p.get("turn"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let latency = event
                .payload
                .as_ref()
                .and_then(|p| p.get("latency_ms"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let finish = event
                .payload
                .as_ref()
                .and_then(|p| p.get("finish_reason"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let prompt = event
                .payload
                .as_ref()
                .and_then(|p| p.get("prompt_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let completion = event
                .payload
                .as_ref()
                .and_then(|p| p.get("completion_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let total = event
                .payload
                .as_ref()
                .and_then(|p| p.get("total_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cached = event
                .payload
                .as_ref()
                .and_then(|p| p.get("cached_prompt_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cost = event
                .payload
                .as_ref()
                .and_then(|p| p.get("cost_usd"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let credits_remaining = event
                .payload
                .as_ref()
                .and_then(|p| p.get("credits_remaining"))
                .and_then(|v| v.as_f64());
            if is_root_agent {
                state.snapshot.usage.turns = state.snapshot.usage.turns.max(turn);
            }
            if is_root_agent && (prompt > 0 || completion > 0 || total > 0) {
                state.stream_received_usage_event = true;
                state.snapshot.usage.last_prompt_tokens = prompt;
                state.snapshot.session.total_prompt_tokens = state
                    .snapshot
                    .session
                    .total_prompt_tokens
                    .saturating_add(prompt);
                state.snapshot.session.total_completion_tokens = state
                    .snapshot
                    .session
                    .total_completion_tokens
                    .saturating_add(completion);
                state.snapshot.usage.total_tokens = state
                    .snapshot
                    .usage
                    .total_tokens
                    .saturating_add(total.max(prompt.saturating_add(completion)));
            }
            if is_root_agent && cached > 0 {
                state.snapshot.usage.cached_prompt_tokens = state
                    .snapshot
                    .usage
                    .cached_prompt_tokens
                    .saturating_add(cached);
            }
            if cost > 0.0 {
                state.snapshot.usage.cost_usd += cost;
            }
            if let Some(credits) = credits_remaining {
                state.snapshot.usage.credits_remaining = Some(credits);
            }
            format!(
                "agent={agent} depth={handoff_depth} response received | turn={turn} latency={latency}ms finish={finish} tokens={prompt}↑ {completion}↓ total={total}"
            )
        };
        state.lines.push(ChatLine {
            role: "model_event".to_string(),
            text,
            tool_result: None,
        });
        note_conversation_update(state);
    }

    if event.kind == "tool" {
        let agent = event
            .payload
            .as_ref()
            .and_then(|p| p.get("agent"))
            .and_then(|v| v.as_str())
            .unwrap_or("root");
        let handoff_depth = event
            .payload
            .as_ref()
            .and_then(|p| p.get("handoff_depth"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let is_root_agent = handoff_depth == 0;
        let tool_name = event
            .payload
            .as_ref()
            .and_then(|p| p.get("tool_name"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| event.title.strip_prefix("tool: ").unwrap_or(&event.title))
            .to_string();
        let phase = event
            .payload
            .as_ref()
            .and_then(|p| p.get("phase"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if phase == "start" {
            if is_root_agent {
                state.snapshot.usage.tool_calls = state.snapshot.usage.tool_calls.saturating_add(1);
            }
            let arguments = event
                .payload
                .as_ref()
                .and_then(|p| p.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let text = format!(
                "agent={agent} depth={handoff_depth}\n{}",
                format_tool_call_line(&tool_name, arguments)
            );
            state.lines.push(ChatLine {
                role: "tool_call".to_string(),
                text,
                tool_result: None,
            });
            note_conversation_update(state);
        } else if phase == "finish" {
            let result = event
                .payload
                .as_ref()
                .and_then(|p| p.get("result"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let success = event
                .payload
                .as_ref()
                .and_then(|p| p.get("success"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let latency_ms = event
                .payload
                .as_ref()
                .and_then(|p| p.get("latency_ms"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            state.tool_results.push(ToolResultLine {
                tool_name: format!("{} [{} d={}]", tool_name, agent, handoff_depth),
                result,
                success,
                latency_ms,
                created_at: Local::now().to_rfc3339(),
            });
            state.lines.push(ChatLine {
                role: "tool_result".to_string(),
                text: String::new(),
                tool_result: state.tool_results.last().cloned(),
            });
            note_conversation_update(state);
            trim_log(&mut state.tool_results, 200);
        }
    }
    state
        .activity
        .push(stamp(&format!("{}: {}", event.title, event.detail)));
    trim_log(&mut state.activity, 200);
}

pub fn apply_stream_chunk(state: &mut UiState, live_output: &mut String, chunk: &StreamChunk) {
    match chunk.kind.as_str() {
        "snapshot" => {
            if let Some(snapshot) = &chunk.snapshot {
                let had_lines = !state.lines.is_empty();
                state.snapshot = snapshot.clone();
                sync_conversation_from_snapshot(state);
                // Keep history navigable when snapshots arrive after initial bootstrap/reconnect.
                if had_lines {
                    state.conversation_follow = false;
                    state.conversation_needs_autoscroll = false;
                }
            }
        }
        "submit" => {
            if let Some(submitted) = &chunk.submitted {
                let already_present = state
                    .lines
                    .last()
                    .map(|l| l.role == "you" && l.text == *submitted)
                    .unwrap_or(false);
                if !already_present {
                    register_submit(state, submitted);
                }
                state.pending = true;
                live_output.clear();
            }
        }
        "delta" => {
            if let Some(text) = &chunk.text {
                live_output.push_str(text);
                register_stream_delta(state, text);
            }
        }
        "status" | "heartbeat" => {
            if let Some(msg) = &chunk.message {
                set_feedback(state, msg);
            }
        }
        "event" => {
            if let Some(event) = &chunk.event {
                register_stream_event(state, event);
            }
        }
        "final" => {
            state.pending = false;
            live_output.clear();
            if let Some(resp) = chunk.response.clone() {
                register_response(state, resp);
            } else {
                register_error(state, "stream final chunk missing response");
            }
        }
        "error" => {
            state.pending = false;
            live_output.clear();
            register_error(
                state,
                chunk.message.as_deref().unwrap_or("unknown stream error"),
            );
        }
        _ => {}
    }
}

pub fn sync_conversation_from_snapshot(state: &mut UiState) {
    let mut tool_results_iter = state.snapshot.tool_results.iter().cloned();
    state.lines = state
        .snapshot
        .conversation
        .iter()
        .map(|line| ChatLine {
            role: line.role.clone(),
            text: line.text.clone(),
            tool_result: None,
        })
        .flat_map(|line| {
            if line.role == "tool_call" {
                let mut v = vec![line];
                if let Some(tool) = tool_results_iter.next() {
                    v.push(ChatLine {
                        role: "tool_result".to_string(),
                        text: String::new(),
                        tool_result: Some(tool),
                    });
                }
                v
            } else {
                vec![line]
            }
        })
        .collect();
    state.tool_results = state.snapshot.tool_results.clone();
    note_conversation_update(state);
}

pub fn note_conversation_update(state: &mut UiState) {
    if state.conversation_follow {
        state.conversation_needs_autoscroll = true;
        state.conversation_unseen = 0;
    } else {
        state.conversation_unseen = 1;
    }
}

pub fn update_conversation_follow_state(state: &mut UiState, max_scroll: u16) {
    if state.conversation_scroll >= max_scroll {
        state.conversation_follow = true;
        state.conversation_unseen = 0;
    } else {
        state.conversation_follow = false;
    }
}

#[derive(Debug, Clone)]
pub enum ConversationSegment {
    Separator,
    Message {
        role: String,
        text: String,
    },
    ToolResult {
        tool: ToolResultLine,
        expanded: bool,
    },
    LiveAssistant {
        text: String,
    },
}

pub fn render_conversation_text(
    lines: &[ChatLine],
    show_tool_result_details: bool,
    pending: bool,
    live_output: &str,
) -> Text<'static> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for segment in
        build_conversation_segments(lines, show_tool_result_details, pending, live_output)
    {
        match segment {
            ConversationSegment::Separator => out.push(Line::from(Span::styled(
                "──────────────────────── next prompt ────────────────────────",
                Style::default().fg(Color::DarkGray),
            ))),
            ConversationSegment::Message { role, text } => {
                if role == "file_diff" {
                    for line in render_file_diff_lines(&text) {
                        out.push(line);
                    }
                    out.push(Line::default());
                    continue;
                }
                let (label, label_style, body_style) = match role.as_str() {
                    "you" => (
                        "YOU",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(210, 245, 255)),
                    ),
                    "assistant" => (
                        "ASSISTANT",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(220, 255, 220)),
                    ),
                    "tool_call" => (
                        "🔧 TOOL",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(245, 210, 255)),
                    ),
                    "system" => (
                        "SYSTEM",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(255, 245, 205)),
                    ),
                    "reasoning" => (
                        "REASONING",
                        Style::default()
                            .fg(Color::LightMagenta)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(240, 220, 255)),
                    ),
                    "model_event" => (
                        "MODEL",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(205, 225, 255)),
                    ),
                    "error" => (
                        "ERROR",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::Rgb(255, 210, 210)),
                    ),
                    _ => (
                        role.as_str(),
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                        Style::default().fg(Color::White),
                    ),
                };
                out.push(Line::from(Span::styled(format!("[{label}]"), label_style)));
                if role == "you" || role == "assistant" {
                    for l in markdown_lines(&text) {
                        out.push(l);
                    }
                } else {
                    for l in text.lines() {
                        out.push(Line::from(Span::styled(format!("  {l}"), body_style)));
                    }
                }
                out.push(Line::default());
            }
            ConversationSegment::ToolResult { tool, expanded } => {
                let status_style = if tool.success {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                };
                let arrow = if expanded { "▼" } else { "▶" };
                out.push(Line::from(vec![
                    Span::styled(
                        format!("[{arrow} TOOL RESULT] "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "{} {} ({}ms)",
                            if tool.success { "ok" } else { "fail" },
                            tool.tool_name,
                            tool.latency_ms
                        ),
                        status_style,
                    ),
                ]));
                if expanded {
                    for l in tool.result.lines() {
                        out.push(Line::from(Span::styled(
                            format!("  {l}"),
                            Style::default().fg(Color::Gray),
                        )));
                    }
                } else {
                    out.push(Line::from(Span::styled(
                        "  (Ctrl+T to expand)",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                out.push(Line::default());
            }
            ConversationSegment::LiveAssistant { text } => {
                out.push(Line::from(Span::styled(
                    "[ASSISTANT …]",
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD),
                )));
                for l in text.lines() {
                    out.push(Line::from(Span::styled(
                        format!("  {l}"),
                        Style::default().fg(Color::Rgb(220, 255, 220)),
                    )));
                }
                out.push(Line::default());
            }
        }
    }
    Text::from(out)
}

fn markdown_lines(input: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::new(ZagentMarkdownStyleSheet);
    let parsed = from_str_with_options(input, &options);
    parsed
        .lines
        .into_iter()
        .map(|line| {
            let mut rendered = Line::from(
                line.spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect::<Vec<_>>(),
            )
            .style(line.style);
            if let Some(alignment) = line.alignment {
                rendered = rendered.alignment(alignment);
            }
            rendered
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
struct ZagentMarkdownStyleSheet;

impl StyleSheet for ZagentMarkdownStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
            2 => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            3 => Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
            _ => Style::default().fg(Color::LightBlue),
        }
    }

    fn code(&self) -> Style {
        Style::default()
            .fg(Color::Yellow)
            .bg(Color::Rgb(30, 30, 30))
    }

    fn link(&self) -> Style {
        Style::default()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::default().fg(Color::LightGreen)
    }

    fn heading_meta(&self) -> Style {
        Style::default().fg(Color::DarkGray)
    }

    fn metadata_block(&self) -> Style {
        Style::default().fg(Color::Gray)
    }
}

fn render_file_diff_lines(payload: &str) -> Vec<Line<'static>> {
    let mut lines = payload.lines();
    let path_line = lines.next().unwrap_or("path:(unknown)");
    let lang_line = lines.next().unwrap_or("lang:text");
    let path = path_line.strip_prefix("path:").unwrap_or(path_line);
    let lang = lang_line.strip_prefix("lang:").unwrap_or("text");

    let mut out: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(
            "[DIFF] ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{path} ({lang})"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ])];

    for raw in lines {
        if raw.starts_with("@@") {
            out.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::LightCyan),
            )));
            continue;
        }
        if let Some(body) = raw.strip_prefix('+') {
            let mut spans = vec![Span::styled(
                "+".to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )];
            spans.extend(highlight_code_line(
                body,
                lang,
                Style::default().fg(Color::Rgb(180, 255, 180)),
            ));
            out.push(Line::from(spans));
            continue;
        }
        if let Some(body) = raw.strip_prefix('-') {
            let mut spans = vec![Span::styled(
                "-".to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )];
            spans.extend(highlight_code_line(
                body,
                lang,
                Style::default().fg(Color::Rgb(255, 180, 180)),
            ));
            out.push(Line::from(spans));
            continue;
        }
        out.push(Line::from(Span::styled(
            raw.to_string(),
            Style::default().fg(Color::Gray),
        )));
    }
    out
}

fn highlight_code_line(line: &str, lang: &str, base: Style) -> Vec<Span<'static>> {
    let keywords: &[&str] = match lang {
        "rust" => &[
            "fn", "let", "mut", "pub", "impl", "struct", "enum", "use", "match", "if", "else",
            "for", "while", "async", "await", "return", "Self", "self", "mod", "crate", "const",
        ],
        "python" => &[
            "def", "class", "import", "from", "if", "elif", "else", "for", "while", "return",
            "with", "as", "try", "except", "finally", "async", "await",
        ],
        "js" | "ts" | "tsx" | "jsx" => &[
            "function",
            "const",
            "let",
            "var",
            "class",
            "if",
            "else",
            "for",
            "while",
            "return",
            "import",
            "from",
            "export",
            "async",
            "await",
            "interface",
            "type",
        ],
        _ => &[],
    };

    let mut spans = Vec::new();
    let mut word = String::new();
    for ch in line.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            let style = if keywords.iter().any(|kw| *kw == word) {
                base.add_modifier(Modifier::BOLD).fg(Color::Yellow)
            } else {
                base
            };
            spans.push(Span::styled(word.clone(), style));
            word.clear();
        }
        spans.push(Span::styled(ch.to_string(), base));
    }
    if !word.is_empty() {
        let style = if keywords.iter().any(|kw| *kw == word) {
            base.add_modifier(Modifier::BOLD).fg(Color::Yellow)
        } else {
            base
        };
        spans.push(Span::styled(word, style));
    }
    spans
}

pub fn build_conversation_segments(
    lines: &[ChatLine],
    show_tool_result_details: bool,
    pending: bool,
    live_output: &str,
) -> Vec<ConversationSegment> {
    let mut segments = Vec::new();
    for line in lines {
        if line.role == "you" {
            segments.push(ConversationSegment::Separator);
        }
        if line.role == "tool_result" {
            if let Some(tool) = &line.tool_result {
                segments.push(ConversationSegment::ToolResult {
                    tool: tool.clone(),
                    expanded: show_tool_result_details,
                });
            }
            continue;
        }
        segments.push(ConversationSegment::Message {
            role: line.role.clone(),
            text: line.text.clone(),
        });
    }
    if pending && !live_output.is_empty() {
        segments.push(ConversationSegment::LiveAssistant {
            text: live_output.to_string(),
        });
    }
    segments
}

pub fn latest_feedback(state: &UiState) -> String {
    state
        .activity
        .last()
        .cloned()
        .unwrap_or_else(|| stamp("Ready"))
}

pub fn tail_single_line(text: &str, max_chars: usize) -> String {
    let single = text.replace('\n', " ");
    let chars: Vec<char> = single.chars().collect();
    if chars.len() <= max_chars {
        return single;
    }
    chars[chars.len() - max_chars..].iter().collect()
}

pub fn scroll_line(text: &str, tick: u64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let sep = "   ";
    let looped = format!("{text}{sep}{text}");
    let looped_chars: Vec<char> = looped.chars().collect();
    let cycle = chars.len() + sep.chars().count();
    let start = (tick as usize) % cycle;
    looped_chars.iter().skip(start).take(width).collect()
}

fn trim_log<T>(log: &mut Vec<T>, max: usize) {
    if log.len() > max {
        let drop_n = log.len() - max;
        log.drain(0..drop_n);
    }
}

fn infer_language(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or_default();
    match ext {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "ts" => "ts",
        "tsx" => "tsx",
        "js" => "js",
        "jsx" => "jsx",
        "py" => "python",
        "sh" => "bash",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        _ => "text",
    }
}

fn format_tool_call_line(tool_name: &str, arguments: &str) -> String {
    if tool_name == "shell_exec"
        && let Some(line) = format_shell_exec_call(arguments)
    {
        return line;
    }
    let pretty_args = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| arguments.to_string());
    format!("🔧 {tool_name}\nargs:\n{pretty_args}")
}

fn format_shell_exec_call(arguments: &str) -> Option<String> {
    let args = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let mut parts = vec![format!("command={}", shell_quote(command))];
    if let Some(working_dir) = args
        .get("working_dir")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("working_dir={}", shell_quote(working_dir)));
    }
    if let Some(timeout) = args.get("timeout_secs").and_then(|v| v.as_u64())
        && timeout != 60
    {
        parts.push(format!("timeout_secs={timeout}"));
    }
    Some(format!("🔧 shell_exec {}", parts.join(" ")))
}

fn shell_quote(value: &str) -> String {
    let compact = value.replace('\n', "\\n");
    format!("\"{}\"", compact.replace('"', "\\\""))
}
