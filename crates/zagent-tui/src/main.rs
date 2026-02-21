use std::io;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use clap::Parser;
use crossterm::event;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use zagent_ui_shared::{
    BackendMessageResponse, BackendSnapshot, ConversationSegment, McpStatusSnapshot, PanelFocus,
    SessionDialogState, SessionListItem, StreamChunk, UiState, activity_text, apply_stream_chunk,
    build_conversation_segments, content_max_scroll, help_dialog_text, input_backspace,
    input_clear, input_delete, input_insert_char, input_insert_text, input_move_left,
    input_move_right, latest_feedback, prompt_text, register_error, register_response,
    register_submit, render_conversation_text, scroll_line, selected_session_index,
    session_dialog_entries_len, session_dialog_text, set_feedback, status_line,
    sync_conversation_from_snapshot, tail_single_line, update_conversation_follow_state,
    usage_metrics_line,
};

#[derive(Debug, Parser)]
#[command(
    name = "zagent-tui",
    version,
    about = "Ratatui frontend for zagent-server"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    backend_url: String,
}

#[derive(Debug, Serialize)]
struct MessageRequest {
    input: String,
}

#[derive(Debug, Serialize)]
struct SetModelRequest {
    provider: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct ContinueSessionRequest {
    name_or_id: String,
}

#[derive(Debug, Serialize)]
struct NewSessionRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionsApiResponse {
    sessions: Vec<SessionListItem>,
}

#[derive(Debug, Deserialize, Clone)]
struct ProviderModel {
    id: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ModelProviderCatalog {
    provider: String,
    models: Vec<ProviderModel>,
}

#[derive(Debug, Deserialize, Clone)]
struct ModelCatalogSnapshot {
    current_provider: String,
    current_model: String,
    providers: Vec<ModelProviderCatalog>,
}

#[derive(Debug, Clone)]
struct ModelChoice {
    provider: String,
    model: String,
    label: String,
}

#[derive(Debug, Default, Clone)]
struct ModelSelectorState {
    visible: bool,
    loading: bool,
    selected: usize,
    filter: String,
    current_provider: String,
    current_model: String,
    items: Vec<ModelChoice>,
}

#[derive(Debug, Default, Clone)]
struct McpDialogState {
    visible: bool,
    loading: bool,
    runtime: String,
    lines: Vec<String>,
}

enum UiMsg {
    Stream(StreamChunk),
    Response(BackendMessageResponse),
    ModelCatalog(ModelCatalogSnapshot),
    McpStatus(McpStatusSnapshot),
    Sessions(Vec<SessionListItem>),
    Error(String),
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let client = reqwest::Client::new();
    let mut clipboard = Clipboard::new().ok();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    execute!(stdout, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut ui = UiState::default();
    let mut pending_since: Option<Instant> = None;
    let mut quit_confirm_until: Option<Instant> = None;
    let mut tick: u64 = 0;
    let mut live_output = String::new();
    let mut model_selector = ModelSelectorState::default();
    let mut session_dialog = SessionDialogState::default();
    let mut mcp_dialog = McpDialogState::default();
    let mut help_dialog_visible = false;
    let (tx, mut rx) = mpsc::unbounded_channel::<UiMsg>();
    match fetch_state(&client, &cli.backend_url).await {
        Ok(snapshot) => {
            ui.snapshot = snapshot;
            sync_conversation_from_snapshot(&mut ui);
        }
        Err(err) => register_error(&mut ui, &err.to_string()),
    }
    {
        let tx = tx.clone();
        let client = client.clone();
        let backend_url = cli.backend_url.clone();
        tokio::spawn(async move {
            if let Err(e) = stream_events_loop(&client, &backend_url, &tx).await {
                let _ = tx.send(UiMsg::Error(e.to_string()));
            }
        });
    }

    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                UiMsg::Stream(chunk) => {
                    apply_stream_chunk(&mut ui, &mut live_output, &chunk);
                    if matches!(chunk.kind.as_str(), "final" | "error") {
                        pending_since = None;
                    }
                }
                UiMsg::Response(resp) => {
                    register_response(&mut ui, resp);
                }
                UiMsg::ModelCatalog(catalog) => {
                    let mut items = Vec::new();
                    for provider in catalog.providers {
                        for model in provider.models {
                            let label = model.name.clone().unwrap_or_else(|| model.id.clone());
                            items.push(ModelChoice {
                                provider: provider.provider.clone(),
                                model: model.id,
                                label,
                            });
                        }
                    }
                    items.sort_by(|a, b| {
                        a.provider
                            .cmp(&b.provider)
                            .then_with(|| a.model.cmp(&b.model))
                    });
                    let selected = items
                        .iter()
                        .position(|m| {
                            m.provider == catalog.current_provider
                                && m.model == catalog.current_model
                        })
                        .unwrap_or(0);
                    model_selector.visible = true;
                    model_selector.loading = false;
                    model_selector.filter.clear();
                    model_selector.selected = selected;
                    model_selector.current_provider = catalog.current_provider;
                    model_selector.current_model = catalog.current_model;
                    model_selector.items = items;
                }
                UiMsg::McpStatus(snapshot) => {
                    let mut lines = Vec::new();
                    for server in snapshot.servers {
                        let mut line = format!(
                            "{} enabled={} state={}",
                            server.name, server.enabled, server.state
                        );
                        if let Some(count) = server.tool_count {
                            line.push_str(&format!(" tools={count}"));
                        }
                        if let Some(err) = server.error {
                            line.push_str(&format!(" error={err}"));
                        }
                        lines.push(line);
                    }
                    mcp_dialog.visible = true;
                    mcp_dialog.loading = false;
                    mcp_dialog.runtime = snapshot.runtime;
                    mcp_dialog.lines = lines;
                }
                UiMsg::Sessions(sessions) => {
                    session_dialog.visible = true;
                    session_dialog.loading = false;
                    session_dialog.selected = 0;
                    session_dialog.current_session_id = ui.snapshot.session.id.clone();
                    session_dialog.items = sessions;
                }
                UiMsg::Error(err) => {
                    ui.pending = false;
                    pending_since = None;
                    live_output.clear();
                    if model_selector.loading {
                        model_selector.visible = false;
                        model_selector.loading = false;
                    }
                    if session_dialog.loading {
                        session_dialog.visible = false;
                        session_dialog.loading = false;
                    }
                    if mcp_dialog.loading {
                        mcp_dialog.visible = false;
                        mcp_dialog.loading = false;
                    }
                    register_error(&mut ui, &err);
                }
            }
        }

        let area: Rect = terminal.size()?.into();
        let conv_max = conversation_max_scroll(&ui, &live_output, area);
        if ui.conversation_follow || ui.conversation_needs_autoscroll {
            ui.conversation_scroll = conv_max;
            ui.conversation_follow = true;
            ui.conversation_unseen = 0;
            ui.conversation_needs_autoscroll = false;
        } else {
            ui.conversation_scroll = ui.conversation_scroll.min(conv_max);
            update_conversation_follow_state(&mut ui, conv_max);
        }

        terminal.draw(|f| {
            render_ui(
                f,
                &ui,
                pending_since,
                tick,
                &live_output,
                &model_selector,
                &session_dialog,
                &mcp_dialog,
                help_dialog_visible,
            )
        })?;
        tick = tick.wrapping_add(1);

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }

                    let is_ctrl_c_quit =
                        matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::SHIFT)
                            && !key.modifiers.contains(KeyModifiers::ALT);
                    if is_ctrl_c_quit {
                        let now = Instant::now();
                        if let Some(until) = quit_confirm_until
                            && now <= until
                        {
                            break;
                        }
                        quit_confirm_until = Some(now + Duration::from_secs(2));
                        set_feedback(
                            &mut ui,
                            "Press Ctrl+C again within 2s to quit. Any other key cancels.",
                        );
                        continue;
                    }
                    if quit_confirm_until.is_some() {
                        quit_confirm_until = None;
                    }

                    match key.code {
                        _ if help_dialog_visible => match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                help_dialog_visible = false;
                            }
                            _ => {}
                        },
                        _ if session_dialog.visible => match key.code {
                            KeyCode::Esc => {
                                session_dialog.visible = false;
                                session_dialog.loading = false;
                                session_dialog.filter.clear();
                                session_dialog.items.clear();
                            }
                            KeyCode::Char(c) => {
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT)
                                {
                                    session_dialog.filter.push(c);
                                    session_dialog.selected = 0;
                                }
                            }
                            KeyCode::Backspace => {
                                session_dialog.filter.pop();
                                session_dialog.selected = 0;
                            }
                            KeyCode::Delete => {
                                session_dialog.filter.clear();
                                session_dialog.selected = 0;
                            }
                            KeyCode::Up => {
                                let max = session_dialog_entries_len(&session_dialog);
                                if max > 0 {
                                    session_dialog.selected =
                                        session_dialog.selected.saturating_sub(1);
                                }
                            }
                            KeyCode::Down => {
                                let max = session_dialog_entries_len(&session_dialog);
                                if max > 0 {
                                    session_dialog.selected =
                                        (session_dialog.selected + 1).min(max - 1);
                                }
                            }
                            KeyCode::Enter => {
                                let tx = tx.clone();
                                let client = client.clone();
                                let backend_url = cli.backend_url.clone();
                                if let Some(idx) = selected_session_index(&session_dialog) {
                                    if let Some(item) = session_dialog.items.get(idx).cloned() {
                                        session_dialog.visible = false;
                                        session_dialog.loading = false;
                                        session_dialog.filter.clear();
                                        session_dialog.items.clear();
                                        tokio::spawn(async move {
                                            match continue_session(&client, &backend_url, &item.id)
                                                .await
                                            {
                                                Ok(resp) => {
                                                    let _ = tx.send(UiMsg::Response(resp));
                                                }
                                                Err(e) => {
                                                    let _ = tx.send(UiMsg::Error(e.to_string()));
                                                }
                                            }
                                        });
                                    }
                                } else {
                                    let name = session_dialog.filter.trim().to_string();
                                    session_dialog.visible = false;
                                    session_dialog.loading = false;
                                    session_dialog.filter.clear();
                                    session_dialog.items.clear();
                                    tokio::spawn(async move {
                                        let maybe_name =
                                            if name.is_empty() { None } else { Some(name) };
                                        match create_session(&client, &backend_url, maybe_name)
                                            .await
                                        {
                                            Ok(resp) => {
                                                let _ = tx.send(UiMsg::Response(resp));
                                            }
                                            Err(e) => {
                                                let _ = tx.send(UiMsg::Error(e.to_string()));
                                            }
                                        }
                                    });
                                }
                            }
                            _ => {}
                        },
                        _ if mcp_dialog.visible => match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                mcp_dialog.visible = false;
                                mcp_dialog.loading = false;
                                mcp_dialog.lines.clear();
                            }
                            _ => {}
                        },
                        _ if model_selector.visible => match key.code {
                            KeyCode::Esc => {
                                model_selector.visible = false;
                                model_selector.loading = false;
                                model_selector.filter.clear();
                                model_selector.items.clear();
                            }
                            KeyCode::Char(c) => {
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT)
                                {
                                    model_selector.filter.push(c);
                                    model_selector.selected = 0;
                                }
                            }
                            KeyCode::Backspace => {
                                model_selector.filter.pop();
                                model_selector.selected = 0;
                            }
                            KeyCode::Delete => {
                                model_selector.filter.clear();
                                model_selector.selected = 0;
                            }
                            KeyCode::Up => {
                                let filtered = filtered_model_indices(&model_selector);
                                if !filtered.is_empty() {
                                    model_selector.selected =
                                        model_selector.selected.saturating_sub(1);
                                }
                            }
                            KeyCode::Down => {
                                let filtered = filtered_model_indices(&model_selector);
                                if !filtered.is_empty() {
                                    model_selector.selected =
                                        (model_selector.selected + 1).min(filtered.len() - 1);
                                }
                            }
                            KeyCode::Enter => {
                                let filtered = filtered_model_indices(&model_selector);
                                if let Some(idx) = filtered.get(model_selector.selected).copied()
                                    && let Some(choice) = model_selector.items.get(idx).cloned()
                                {
                                    model_selector.visible = false;
                                    model_selector.loading = false;
                                    model_selector.filter.clear();
                                    model_selector.items.clear();
                                    let tx = tx.clone();
                                    let client = client.clone();
                                    let backend_url = cli.backend_url.clone();
                                    tokio::spawn(async move {
                                        match set_model_selection(
                                            &client,
                                            &backend_url,
                                            &choice.provider,
                                            &choice.model,
                                        )
                                        .await
                                        {
                                            Ok(resp) => {
                                                let _ = tx.send(UiMsg::Response(resp));
                                            }
                                            Err(e) => {
                                                let _ = tx.send(UiMsg::Error(e.to_string()));
                                            }
                                        }
                                    });
                                }
                            }
                            _ => {}
                        },
                        KeyCode::Char(c) => {
                            let mods = key.modifiers;
                            if mods.contains(KeyModifiers::CONTROL) && (c == 't' || c == 'T') {
                                ui.show_tool_result_details = !ui.show_tool_result_details;
                                let state_label = if ui.show_tool_result_details {
                                    "expanded"
                                } else {
                                    "collapsed"
                                };
                                set_feedback(
                                    &mut ui,
                                    &format!("Inline tool results {state_label}."),
                                );
                                continue;
                            }

                            if mods.contains(KeyModifiers::CONTROL)
                                && mods.contains(KeyModifiers::SHIFT)
                                && (c == 'c' || c == 'C')
                            {
                                if let Some(ref mut cb) = clipboard {
                                    match cb.set_text(ui.input.clone()) {
                                        Ok(_) => {
                                            set_feedback(&mut ui, "Copied input to clipboard.")
                                        }
                                        Err(e) => set_feedback(
                                            &mut ui,
                                            &format!("Clipboard copy failed: {e}"),
                                        ),
                                    }
                                } else {
                                    set_feedback(&mut ui, "Clipboard unavailable.");
                                }
                                continue;
                            }

                            if mods.contains(KeyModifiers::CONTROL)
                                && mods.contains(KeyModifiers::SHIFT)
                                && (c == 'v' || c == 'V')
                            {
                                if let Some(ref mut cb) = clipboard {
                                    match cb.get_text() {
                                        Ok(text) => {
                                            input_insert_text(&mut ui, &text);
                                            set_feedback(&mut ui, "Pasted clipboard into input.");
                                        }
                                        Err(e) => set_feedback(
                                            &mut ui,
                                            &format!("Clipboard paste failed: {e}"),
                                        ),
                                    }
                                } else {
                                    set_feedback(&mut ui, "Clipboard unavailable.");
                                }
                                continue;
                            }

                            if !mods.contains(KeyModifiers::CONTROL)
                                && !mods.contains(KeyModifiers::ALT)
                            {
                                input_insert_char(&mut ui, c);
                            }
                        }
                        KeyCode::Backspace => {
                            input_backspace(&mut ui);
                        }
                        KeyCode::Delete => {
                            input_delete(&mut ui);
                        }
                        KeyCode::Left => {
                            input_move_left(&mut ui);
                        }
                        KeyCode::Right => {
                            input_move_right(&mut ui);
                        }
                        KeyCode::Tab => {
                            ui.focus = ui.focus.next();
                            let label = ui.focus.label();
                            set_feedback(&mut ui, &format!("Focused panel: {label}"));
                        }
                        KeyCode::Up => {
                            let max = max_active_scroll(&ui, &live_output, terminal.size()?.into());
                            scroll_active_panel_with_max(&mut ui, -1, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(
                                    &ui,
                                    &live_output,
                                    terminal.size()?.into(),
                                );
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        KeyCode::Down => {
                            let max = max_active_scroll(&ui, &live_output, terminal.size()?.into());
                            scroll_active_panel_with_max(&mut ui, 1, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(
                                    &ui,
                                    &live_output,
                                    terminal.size()?.into(),
                                );
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        KeyCode::PageUp => {
                            let max = max_active_scroll(&ui, &live_output, terminal.size()?.into());
                            scroll_active_panel_with_max(&mut ui, -8, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(
                                    &ui,
                                    &live_output,
                                    terminal.size()?.into(),
                                );
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        KeyCode::PageDown => {
                            let max = max_active_scroll(&ui, &live_output, terminal.size()?.into());
                            scroll_active_panel_with_max(&mut ui, 8, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(
                                    &ui,
                                    &live_output,
                                    terminal.size()?.into(),
                                );
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        KeyCode::Home => {
                            set_active_scroll(&mut ui, 0);
                            if ui.focus == PanelFocus::Conversation {
                                ui.conversation_follow = false;
                            }
                        }
                        KeyCode::End => {
                            let max = max_active_scroll(&ui, &live_output, terminal.size()?.into());
                            set_active_scroll(&mut ui, max);
                            if ui.focus == PanelFocus::Conversation {
                                ui.conversation_follow = true;
                                ui.conversation_unseen = 0;
                                ui.conversation_needs_autoscroll = false;
                            }
                        }
                        KeyCode::Enter => {
                            if key.modifiers.contains(KeyModifiers::CONTROL) {
                                input_insert_char(&mut ui, '\n');
                                continue;
                            }
                            if ui.pending {
                                set_feedback(
                                    &mut ui,
                                    "Agent is still running. Wait for completion.",
                                );
                                continue;
                            }

                            let submitted = ui.input.clone();
                            input_clear(&mut ui);
                            if submitted.trim().is_empty() {
                                continue;
                            }
                            if submitted.trim() == "/model" {
                                model_selector.visible = true;
                                model_selector.loading = true;
                                model_selector.selected = 0;
                                model_selector.filter.clear();
                                model_selector.items.clear();
                                let tx = tx.clone();
                                let client = client.clone();
                                let backend_url = cli.backend_url.clone();
                                tokio::spawn(async move {
                                    match fetch_models_catalog(&client, &backend_url).await {
                                        Ok(catalog) => {
                                            let _ = tx.send(UiMsg::ModelCatalog(catalog));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(UiMsg::Error(e.to_string()));
                                        }
                                    }
                                });
                                continue;
                            }
                            if submitted.trim() == "/session" {
                                session_dialog.visible = true;
                                session_dialog.loading = true;
                                session_dialog.selected = 0;
                                session_dialog.filter.clear();
                                session_dialog.current_session_id = ui.snapshot.session.id.clone();
                                session_dialog.items.clear();
                                let tx = tx.clone();
                                let client = client.clone();
                                let backend_url = cli.backend_url.clone();
                                tokio::spawn(async move {
                                    match fetch_sessions(&client, &backend_url).await {
                                        Ok(sessions) => {
                                            let _ = tx.send(UiMsg::Sessions(sessions));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(UiMsg::Error(e.to_string()));
                                        }
                                    }
                                });
                                continue;
                            }
                            if submitted.trim() == "/help" || submitted.trim() == "/h" {
                                help_dialog_visible = true;
                                continue;
                            }
                            if submitted.trim() == "/mcp" {
                                mcp_dialog.visible = true;
                                mcp_dialog.loading = true;
                                mcp_dialog.lines.clear();
                                let tx = tx.clone();
                                let client = client.clone();
                                let backend_url = cli.backend_url.clone();
                                tokio::spawn(async move {
                                    match fetch_mcp_status(&client, &backend_url).await {
                                        Ok(snapshot) => {
                                            let _ = tx.send(UiMsg::McpStatus(snapshot));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(UiMsg::Error(e.to_string()));
                                        }
                                    }
                                });
                                continue;
                            }
                            if submitted.trim() == "/quit" || submitted.trim() == "/q" {
                                break;
                            }

                            register_submit(&mut ui, &submitted);
                            ui.pending = true;
                            live_output.clear();
                            pending_since = Some(Instant::now());
                            set_feedback(
                                &mut ui,
                                "Request sent. Waiting for model/tool execution...",
                            );

                            let tx = tx.clone();
                            let client = client.clone();
                            let backend_url = cli.backend_url.clone();
                            tokio::spawn(async move {
                                if let Err(e) =
                                    start_message(&client, &backend_url, &submitted).await
                                {
                                    let _ = tx.send(UiMsg::Error(e.to_string()));
                                }
                            });
                        }
                        KeyCode::Esc => break,
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    let area: Rect = terminal.size()?.into();
                    let root = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(3),
                            Constraint::Min(8),
                            Constraint::Length(3),
                            Constraint::Length(5),
                        ])
                        .split(area);

                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            let max = max_active_scroll(&ui, &live_output, area);
                            scroll_active_panel_with_max(&mut ui, -3, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(&ui, &live_output, area);
                                ui.conversation_follow = false;
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            let max = max_active_scroll(&ui, &live_output, area);
                            scroll_active_panel_with_max(&mut ui, 3, max);
                            if ui.focus == PanelFocus::Conversation {
                                let conv_max = conversation_max_scroll(&ui, &live_output, area);
                                ui.conversation_follow = false;
                                update_conversation_follow_state(&mut ui, conv_max);
                            }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            // Check if click is on scrollbar area (right side of conversation panel)
                            let conv_inner = root[1].inner(Margin {
                                vertical: 1,
                                horizontal: 1,
                            });
                            if mouse.column >= conv_inner.right().saturating_sub(1)
                                && mouse.row >= conv_inner.top()
                                && mouse.row < conv_inner.bottom()
                            {
                                let max = conversation_max_scroll(&ui, &live_output, area);
                                if max > 0 {
                                    let position = ((mouse.row - conv_inner.top()) as f32
                                        / conv_inner.height as f32
                                        * (max as f32 + 1.0))
                                        as u16;
                                    ui.conversation_scroll = position.min(max);
                                    ui.conversation_follow = false;
                                }
                            }
                            // Check if click is on scrollbar area (right side of activity panel)
                            let feedback_inner = root[2].inner(Margin {
                                vertical: 1,
                                horizontal: 1,
                            });
                            if mouse.column >= feedback_inner.right().saturating_sub(1)
                                && mouse.row >= feedback_inner.top()
                                && mouse.row < feedback_inner.bottom()
                            {
                                let feedback_text = format!(
                                    "{}\n{}",
                                    latest_feedback(&ui),
                                    activity_text(&ui.activity)
                                );
                                let max = content_max_scroll(
                                    &feedback_text,
                                    root[2].height.saturating_sub(2),
                                );
                                if max > 0 {
                                    let position = ((mouse.row - feedback_inner.top()) as f32
                                        / feedback_inner.height as f32
                                        * (max as f32 + 1.0))
                                        as u16;
                                    ui.activity_scroll = position.min(max);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), DisableMouseCapture)?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn render_ui(
    f: &mut Frame,
    ui: &UiState,
    pending_since: Option<Instant>,
    tick: u64,
    live_output: &str,
    model_selector: &ModelSelectorState,
    session_dialog: &SessionDialogState,
    mcp_dialog: &McpDialogState,
    help_dialog_visible: bool,
) {
    let panel_title = |name: &str, focus: bool| {
        if focus {
            format!("{name} *")
        } else {
            name.to_string()
        }
    };
    let panel_block = |title: String, focus: bool| {
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .title_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(if focus {
                Style::default().fg(Color::LightCyan)
            } else {
                Style::default().fg(Color::DarkGray)
            })
    };

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(f.area());

    let status = Paragraph::new(status_line(&ui.snapshot))
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .title("zAgent")
                .borders(Borders::ALL)
                .title_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .border_style(Style::default().fg(Color::Gray)),
        );
    f.render_widget(status, root[0]);

    let conv_plain = conversation_render_plain(ui, live_output);
    let conversation_scroll = ui.conversation_scroll.min(content_max_scroll(
        &conv_plain,
        root[1].height.saturating_sub(2),
    ));

    let conv_title = if ui.conversation_unseen > 0 {
        format!("Conversation (+{} new)", ui.conversation_unseen)
    } else {
        "Conversation".to_string()
    };
    let conversation = Paragraph::new(render_conversation_text(
        &ui.lines,
        ui.show_tool_result_details,
        ui.pending,
        live_output,
    ))
    .block(panel_block(
        panel_title(&conv_title, ui.focus == PanelFocus::Conversation),
        ui.focus == PanelFocus::Conversation,
    ))
    .scroll((conversation_scroll, 0))
    .wrap(Wrap { trim: false });
    f.render_widget(conversation, root[1]);
    if content_max_scroll(&conv_plain, root[1].height.saturating_sub(2)) > 0 {
        let conv_inner = root[1].inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let mut conv_scroll_state = ScrollbarState::new(conv_plain.lines().count().max(1))
            .viewport_content_length(root[1].height.saturating_sub(2) as usize)
            .position(conversation_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("▐")
                .track_symbol(None)
                .begin_symbol(None)
                .end_symbol(None),
            conv_inner,
            &mut conv_scroll_state,
        );
    }

    let feedback_raw = if ui.pending {
        let elapsed = pending_since
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or_default();
        let spinner = ["|", "/", "-", "\\"][(tick as usize / 4) % 4];
        if live_output.is_empty() {
            format!("{spinner} model thinking... {:.1}s", elapsed)
        } else {
            format!(
                "{spinner} model output: {}",
                tail_single_line(live_output, 90)
            )
        }
    } else {
        latest_feedback(ui)
    };
    let feedback_head = scroll_line(
        &feedback_raw,
        tick / 2,
        root[2].width.saturating_sub(2) as usize,
    );
    let feedback_text = format!("{feedback_head}\n{}", activity_text(&ui.activity));
    let feedback_scroll = ui.activity_scroll.min(content_max_scroll(
        &feedback_text,
        root[2].height.saturating_sub(2),
    ));
    f.render_widget(
        Paragraph::new(feedback_text.clone())
            .style(Style::default().fg(Color::LightBlue))
            .scroll((feedback_scroll, 0))
            .block(panel_block(
                panel_title("Feedback", ui.focus == PanelFocus::Activity),
                ui.focus == PanelFocus::Activity,
            )),
        root[2],
    );

    if content_max_scroll(&feedback_text, root[2].height.saturating_sub(2)) > 0 {
        let feedback_inner = root[2].inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let mut feedback_scroll_state = ScrollbarState::new(feedback_text.lines().count().max(1))
            .viewport_content_length(root[2].height.saturating_sub(2) as usize)
            .position(feedback_scroll as usize);
        f.render_stateful_widget(
            Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("▐")
                .track_symbol(None)
                .begin_symbol(None)
                .end_symbol(None),
            feedback_inner,
            &mut feedback_scroll_state,
        );
    }

    let input = Paragraph::new(format!(
        "{}\n{}",
        prompt_text(&ui.input, ui.input_cursor, false),
        usage_metrics_line(&ui.snapshot)
    ))
    .style(Style::default().fg(Color::Rgb(255, 240, 170)))
    .wrap(Wrap { trim: false })
    .block(
        Block::default()
            .title("Input Enter=send (/help opens help) Tab=focus")
            .borders(Borders::ALL)
            .title_style(
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )
            .border_style(Style::default().fg(Color::Gray)),
    );
    f.render_widget(input, root[3]);
    if !model_selector.visible
        && !session_dialog.visible
        && !mcp_dialog.visible
        && !help_dialog_visible
    {
        let input_inner = root[3].inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let max_cols = input_inner.width.saturating_sub(3) as usize;
        let cursor_col = ui.input_cursor.min(max_cols) as u16;
        f.set_cursor_position((input_inner.x + 2 + cursor_col, input_inner.y));
    }

    if model_selector.visible {
        let popup = centered_rect(80, 70, f.area());
        f.render_widget(
            Block::default()
                .title("Model Selector")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );
        let inner = popup.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let text = if model_selector.loading {
            "Loading models...".to_string()
        } else if model_selector.items.is_empty() {
            "No models returned by configured providers.".to_string()
        } else {
            let filtered = filtered_model_indices(model_selector);
            let selected = model_selector
                .selected
                .min(filtered.len().saturating_sub(1));
            let mut out = String::new();
            out.push_str(&format!(
                "Filter: {}\nCurrent: {} / {}\n\n",
                model_selector.filter,
                model_selector.current_provider,
                model_selector.current_model
            ));
            if filtered.is_empty() {
                out.push_str("No matches.\n");
                out.push_str("(Type to filter, Delete clears filter)\n");
            } else {
                for (pos, idx) in filtered.iter().copied().enumerate() {
                    let item = &model_selector.items[idx];
                    let marker = if pos == selected { ">" } else { " " };
                    out.push_str(&format!(
                        "{marker} {} / {} ({})\n",
                        item.provider, item.model, item.label
                    ));
                }
            }
            out
        };
        f.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    if session_dialog.visible {
        let popup = centered_rect(85, 70, f.area());
        f.render_widget(
            Block::default()
                .title("Sessions")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );
        let inner = popup.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        f.render_widget(
            Paragraph::new(session_dialog_text(session_dialog))
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    if mcp_dialog.visible {
        let popup = centered_rect(80, 60, f.area());
        f.render_widget(
            Block::default()
                .title("MCP Servers")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );
        let inner = popup.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let text = if mcp_dialog.loading {
            "Loading MCP server status...".to_string()
        } else if mcp_dialog.lines.is_empty() {
            format!("runtime={}\nNo MCP servers configured.", mcp_dialog.runtime)
        } else {
            format!(
                "runtime={}\n\n{}\n\n(Press Esc or Enter to close)",
                mcp_dialog.runtime,
                mcp_dialog.lines.join("\n")
            )
        };
        f.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    if help_dialog_visible {
        let popup = centered_rect(80, 70, f.area());
        f.render_widget(
            Block::default()
                .title("Help")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );
        let inner = popup.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        f.render_widget(
            Paragraph::new(help_dialog_text())
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false }),
            inner,
        );
    }
}

fn filtered_model_indices(selector: &ModelSelectorState) -> Vec<usize> {
    let needle = selector.filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return (0..selector.items.len()).collect();
    }
    selector
        .items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let haystack = format!("{} {} {}", item.provider, item.model, item.label);
            if haystack.to_ascii_lowercase().contains(&needle) {
                Some(idx)
            } else {
                None
            }
        })
        .collect()
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn scroll_active_panel(ui: &mut UiState, delta: i16) {
    let scroll = match ui.focus {
        PanelFocus::Conversation => &mut ui.conversation_scroll,
        PanelFocus::Activity => &mut ui.activity_scroll,
    };
    if delta < 0 {
        *scroll = scroll.saturating_sub(delta.unsigned_abs());
    } else {
        *scroll = scroll.saturating_add(delta as u16);
    }
}

fn scroll_active_panel_with_max(ui: &mut UiState, delta: i16, max: u16) {
    scroll_active_panel(ui, delta);
    set_active_scroll(ui, active_scroll(ui).min(max));
}

fn set_active_scroll(ui: &mut UiState, value: u16) {
    match ui.focus {
        PanelFocus::Conversation => ui.conversation_scroll = value,
        PanelFocus::Activity => ui.activity_scroll = value,
    }
}

fn active_scroll(ui: &UiState) -> u16 {
    match ui.focus {
        PanelFocus::Conversation => ui.conversation_scroll,
        PanelFocus::Activity => ui.activity_scroll,
    }
}

fn max_active_scroll(ui: &UiState, live_output: &str, area: Rect) -> u16 {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(area);
    match ui.focus {
        PanelFocus::Conversation => content_max_scroll(
            &conversation_render_plain(ui, live_output),
            root[1].height.saturating_sub(2),
        ),
        PanelFocus::Activity => {
            let head = if ui.pending {
                if live_output.is_empty() {
                    "model thinking...".to_string()
                } else {
                    format!("model output: {}", tail_single_line(live_output, 90))
                }
            } else {
                latest_feedback(ui)
            };
            let text = format!("{head}\n{}", activity_text(&ui.activity));
            content_max_scroll(&text, root[2].height.saturating_sub(2))
        }
    }
}

fn conversation_max_scroll(ui: &UiState, live_output: &str, area: Rect) -> u16 {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(5),
        ])
        .split(area);
    content_max_scroll(
        &conversation_render_plain(ui, live_output),
        root[1].height.saturating_sub(2),
    )
}

fn conversation_render_plain(ui: &UiState, live_output: &str) -> String {
    let mut out = String::new();
    for segment in build_conversation_segments(
        &ui.lines,
        ui.show_tool_result_details,
        ui.pending,
        live_output,
    ) {
        match segment {
            ConversationSegment::Separator => {
                out.push_str("──────────────────────── next prompt ────────────────────────\n");
            }
            ConversationSegment::Message { role, text } => {
                out.push_str(&format!("[{role}]\n"));
                for l in text.lines() {
                    out.push_str(&format!("  {l}\n"));
                }
                out.push('\n');
            }
            ConversationSegment::ToolResult { tool, expanded } => {
                out.push_str(&format!(
                    "[tool_result] {} {} ({}ms)\n",
                    if tool.success { "ok" } else { "fail" },
                    tool.tool_name,
                    tool.latency_ms
                ));
                if expanded {
                    for l in tool.result.lines() {
                        out.push_str(&format!("  {l}\n"));
                    }
                } else {
                    out.push_str("  (Ctrl+T to expand)\n");
                }
                out.push('\n');
            }
            ConversationSegment::LiveAssistant { text } => {
                out.push_str("[assistant…]\n");
                for l in text.lines() {
                    out.push_str(&format!("  {l}\n"));
                }
                out.push('\n');
            }
        }
    }
    out
}

async fn fetch_state(
    client: &reqwest::Client,
    backend_url: &str,
) -> Result<BackendSnapshot, Box<dyn std::error::Error>> {
    let url = format!("{}/api/state", backend_url.trim_end_matches('/'));
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.json::<BackendSnapshot>().await?)
}

async fn fetch_models_catalog(
    client: &reqwest::Client,
    backend_url: &str,
) -> Result<ModelCatalogSnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/models", backend_url.trim_end_matches('/'));
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.json::<ModelCatalogSnapshot>().await?)
}

async fn fetch_sessions(
    client: &reqwest::Client,
    backend_url: &str,
) -> Result<Vec<SessionListItem>, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/sessions", backend_url.trim_end_matches('/'));
    let resp = client.get(url).send().await?.error_for_status()?;
    let payload = resp.json::<SessionsApiResponse>().await?;
    Ok(payload.sessions)
}

async fn fetch_mcp_status(
    client: &reqwest::Client,
    backend_url: &str,
) -> Result<McpStatusSnapshot, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/mcp", backend_url.trim_end_matches('/'));
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.json::<McpStatusSnapshot>().await?)
}

async fn create_session(
    client: &reqwest::Client,
    backend_url: &str,
    name: Option<String>,
) -> Result<BackendMessageResponse, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/session/new", backend_url.trim_end_matches('/'));
    let resp = client
        .post(url)
        .json(&NewSessionRequest { name })
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json::<BackendMessageResponse>().await?)
}

async fn continue_session(
    client: &reqwest::Client,
    backend_url: &str,
    name_or_id: &str,
) -> Result<BackendMessageResponse, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/session/continue", backend_url.trim_end_matches('/'));
    let resp = client
        .post(url)
        .json(&ContinueSessionRequest {
            name_or_id: name_or_id.to_string(),
        })
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json::<BackendMessageResponse>().await?)
}

async fn set_model_selection(
    client: &reqwest::Client,
    backend_url: &str,
    provider: &str,
    model: &str,
) -> Result<BackendMessageResponse, Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/model", backend_url.trim_end_matches('/'));
    let resp = client
        .post(url)
        .json(&SetModelRequest {
            provider: provider.to_string(),
            model: model.to_string(),
        })
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json::<BackendMessageResponse>().await?)
}

async fn start_message(
    client: &reqwest::Client,
    backend_url: &str,
    input: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/api/message/start", backend_url.trim_end_matches('/'));
    client
        .post(url)
        .json(&MessageRequest {
            input: input.to_string(),
        })
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn stream_events_loop(
    client: &reqwest::Client,
    backend_url: &str,
    tx: &mpsc::UnboundedSender<UiMsg>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        let url = format!("{}/api/events/stream", backend_url.trim_end_matches('/'));
        let resp = client.get(url).send().await?.error_for_status()?;
        if let Err(e) = process_stream_response(resp, tx).await {
            let _ = tx.send(UiMsg::Error(format!("events stream disconnected: {e}")));
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn process_stream_response(
    resp: reqwest::Response,
    tx: &mpsc::UnboundedSender<UiMsg>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(idx) = buffer.find("\n\n") {
            let raw_event = buffer[..idx].to_string();
            buffer = buffer[idx + 2..].to_string();

            let data_lines: Vec<String> = raw_event
                .lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|s| s.trim_start().to_string())
                })
                .collect();
            if data_lines.is_empty() {
                continue;
            }
            let payload = data_lines.join("\n");
            let parsed: StreamChunk = serde_json::from_str(&payload)?;
            let _ = tx.send(UiMsg::Stream(parsed));
        }
    }

    Ok(())
}
