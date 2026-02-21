use std::{cell::RefCell, rc::Rc};

use gloo_net::http::Request;
use ratzilla::ratatui::{
    Terminal,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use ratzilla::web_sys;
use ratzilla::{DomBackend, WebRenderer, event::KeyCode};
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use zagent_ui_shared::{
    BackendMessageResponse, BackendSnapshot, ConversationSegment, McpStatusSnapshot, PanelFocus,
    SessionDialogState, SessionListItem, StreamChunk, UiState, activity_text, apply_stream_chunk,
    build_conversation_segments, content_max_scroll, content_max_scroll_wrapped, help_dialog_text,
    input_backspace, input_clear, input_delete, input_insert_char, input_insert_text,
    input_move_left, input_move_right, latest_feedback, prompt_text, register_error,
    register_response, register_submit, render_conversation_text, scroll_line,
    selected_session_index, session_dialog_entries_len, session_dialog_text, set_feedback,
    status_line, sync_conversation_from_snapshot, tail_single_line,
    update_conversation_follow_state, usage_metrics_line, wrapped_line_count,
};

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

#[derive(Debug, serde::Serialize)]
struct MessageRequest {
    input: String,
}

#[derive(Debug, serde::Serialize)]
struct SetModelRequest {
    provider: String,
    model: String,
}

#[derive(Debug, serde::Serialize)]
struct ContinueSessionRequest {
    name_or_id: String,
}

#[derive(Debug, serde::Serialize)]
struct NewSessionRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionsApiResponse {
    sessions: Vec<SessionListItem>,
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

#[wasm_bindgen(inline_js = r#"
export async function zagentReadClipboard() {
  if (!window.zagentClipboard) throw new Error('clipboard helper missing');
  return await window.zagentClipboard.readText();
}

export async function zagentWriteClipboard(text) {
  if (!window.zagentClipboard) throw new Error('clipboard helper missing');
  await window.zagentClipboard.writeText(text);
  return true;
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn zagentReadClipboard() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn zagentWriteClipboard(text: String) -> Result<JsValue, JsValue>;
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let app = Rc::new(RefCell::new(UiState::default()));
    let model_selector = Rc::new(RefCell::new(ModelSelectorState::default()));
    let session_dialog = Rc::new(RefCell::new(SessionDialogState::default()));
    let mcp_dialog = Rc::new(RefCell::new(McpDialogState::default()));
    let help_dialog_visible = Rc::new(RefCell::new(false));

    {
        let app = app.clone();
        spawn_local(async move {
            match fetch_state().await {
                Ok(snapshot) => {
                    let mut state = app.borrow_mut();
                    state.snapshot = snapshot;
                    sync_conversation_from_snapshot(&mut state);
                }
                Err(err) => register_error(&mut app.borrow_mut(), &err),
            }
        });
    }
    start_events_stream(app.clone());

    let backend = DomBackend::new().map_err(|e| JsValue::from_str(&e.to_string()))?;
    let terminal = Terminal::new(backend).map_err(|e| JsValue::from_str(&e.to_string()))?;

    terminal.on_key_event({
        let app = app.clone();
        let model_selector = model_selector.clone();
        let session_dialog = session_dialog.clone();
        let mcp_dialog = mcp_dialog.clone();
        let help_dialog_visible = help_dialog_visible.clone();
        move |key_event| {
            if *help_dialog_visible.borrow() {
                match key_event.code {
                    KeyCode::Esc | KeyCode::Enter => {
                        *help_dialog_visible.borrow_mut() = false;
                    }
                    _ => {}
                }
                return;
            }
            {
                let mut dialog = session_dialog.borrow_mut();
                if dialog.visible {
                    match key_event.code {
                        KeyCode::Esc => {
                            dialog.visible = false;
                            dialog.loading = false;
                            dialog.filter.clear();
                            dialog.items.clear();
                        }
                        KeyCode::Char(c) => {
                            if !key_event.ctrl && !key_event.alt {
                                dialog.filter.push(c);
                                dialog.selected = 0;
                            }
                        }
                        KeyCode::Backspace => {
                            dialog.filter.pop();
                            dialog.selected = 0;
                        }
                        KeyCode::Delete => {
                            dialog.filter.clear();
                            dialog.selected = 0;
                        }
                        KeyCode::Up => {
                            let max = session_dialog_entries_len(&dialog);
                            if max > 0 {
                                dialog.selected = dialog.selected.saturating_sub(1);
                            }
                        }
                        KeyCode::Down => {
                            let max = session_dialog_entries_len(&dialog);
                            if max > 0 {
                                dialog.selected = (dialog.selected + 1).min(max - 1);
                            }
                        }
                        KeyCode::Enter => {
                            let app = app.clone();
                            if let Some(idx) = selected_session_index(&dialog) {
                                if let Some(item) = dialog.items.get(idx).cloned() {
                                    dialog.visible = false;
                                    dialog.loading = false;
                                    dialog.filter.clear();
                                    dialog.items.clear();
                                    drop(dialog);
                                    spawn_local(async move {
                                        match continue_session(&item.id).await {
                                            Ok(resp) => {
                                                register_response(&mut app.borrow_mut(), resp)
                                            }
                                            Err(err) => register_error(&mut app.borrow_mut(), &err),
                                        }
                                    });
                                }
                            } else {
                                let name = dialog.filter.trim().to_string();
                                dialog.visible = false;
                                dialog.loading = false;
                                dialog.filter.clear();
                                dialog.items.clear();
                                drop(dialog);
                                spawn_local(async move {
                                    let maybe_name =
                                        if name.is_empty() { None } else { Some(name) };
                                    match create_session(maybe_name).await {
                                        Ok(resp) => register_response(&mut app.borrow_mut(), resp),
                                        Err(err) => register_error(&mut app.borrow_mut(), &err),
                                    }
                                });
                            }
                        }
                        _ => {}
                    }
                    return;
                }
            }
            {
                let mut dialog = mcp_dialog.borrow_mut();
                if dialog.visible {
                    match key_event.code {
                        KeyCode::Esc | KeyCode::Enter => {
                            dialog.visible = false;
                            dialog.loading = false;
                            dialog.lines.clear();
                        }
                        _ => {}
                    }
                    return;
                }
            }
            {
                let mut selector = model_selector.borrow_mut();
                if selector.visible {
                    match key_event.code {
                        KeyCode::Esc => {
                            selector.visible = false;
                            selector.loading = false;
                            selector.filter.clear();
                            selector.items.clear();
                            return;
                        }
                        KeyCode::Char(c) => {
                            if !key_event.ctrl && !key_event.alt {
                                selector.filter.push(c);
                                selector.selected = 0;
                            }
                            return;
                        }
                        KeyCode::Backspace => {
                            selector.filter.pop();
                            selector.selected = 0;
                            return;
                        }
                        KeyCode::Delete => {
                            selector.filter.clear();
                            selector.selected = 0;
                            return;
                        }
                        KeyCode::Up => {
                            let filtered = filtered_model_indices(&selector);
                            if !filtered.is_empty() {
                                selector.selected = selector.selected.saturating_sub(1);
                            }
                            return;
                        }
                        KeyCode::Down => {
                            let filtered = filtered_model_indices(&selector);
                            if !filtered.is_empty() {
                                selector.selected = (selector.selected + 1).min(filtered.len() - 1);
                            }
                            return;
                        }
                        KeyCode::Enter => {
                            let filtered = filtered_model_indices(&selector);
                            if let Some(idx) = filtered.get(selector.selected).copied()
                                && let Some(choice) = selector.items.get(idx).cloned()
                            {
                                selector.visible = false;
                                selector.loading = false;
                                selector.filter.clear();
                                selector.items.clear();
                                drop(selector);
                                let app = app.clone();
                                spawn_local(async move {
                                    match set_model_selection(&choice.provider, &choice.model).await
                                    {
                                        Ok(resp) => register_response(&mut app.borrow_mut(), resp),
                                        Err(err) => register_error(&mut app.borrow_mut(), &err),
                                    }
                                });
                            }
                            return;
                        }
                        _ => return,
                    }
                }
            }

            let mut state = app.borrow_mut();
            match key_event.code {
                KeyCode::Char(c) => {
                    if key_event.ctrl && (c == 't' || c == 'T') {
                        state.show_tool_result_details = !state.show_tool_result_details;
                        let state_label = if state.show_tool_result_details {
                            "expanded"
                        } else {
                            "collapsed"
                        };
                        set_feedback(&mut state, &format!("Inline tool results {state_label}."));
                        return;
                    }

                    if key_event.ctrl && key_event.shift && (c == 'c' || c == 'C') {
                        let text = state.input.clone();
                        drop(state);
                        let app = app.clone();
                        spawn_local(async move {
                            match zagentWriteClipboard(text).await {
                                Ok(_) => set_feedback(
                                    &mut app.borrow_mut(),
                                    "Copied input to clipboard.",
                                ),
                                Err(err) => set_feedback(
                                    &mut app.borrow_mut(),
                                    &format!("Clipboard copy failed: {:?}", err),
                                ),
                            }
                        });
                        return;
                    }

                    if key_event.ctrl && key_event.shift && (c == 'v' || c == 'V') {
                        drop(state);
                        let app = app.clone();
                        spawn_local(async move {
                            match zagentReadClipboard().await {
                                Ok(value) => {
                                    let text = value.as_string().unwrap_or_default();
                                    let mut state = app.borrow_mut();
                                    input_insert_text(&mut state, &text);
                                    set_feedback(&mut state, "Pasted clipboard into input.");
                                }
                                Err(err) => set_feedback(
                                    &mut app.borrow_mut(),
                                    &format!("Clipboard paste failed: {:?}", err),
                                ),
                            }
                        });
                        return;
                    }

                    if !key_event.ctrl && !key_event.alt {
                        input_insert_char(&mut state, c);
                    }
                }
                KeyCode::Backspace => {
                    input_backspace(&mut state);
                }
                KeyCode::Delete => {
                    input_delete(&mut state);
                }
                KeyCode::Left => {
                    input_move_left(&mut state);
                }
                KeyCode::Right => {
                    input_move_right(&mut state);
                }
                KeyCode::Tab => {
                    state.focus = state.focus.next();
                    let label = state.focus.label();
                    set_feedback(&mut state, &format!("Focused panel: {label}"));
                }
                KeyCode::Up => {
                    scroll_active_panel(&mut state, -1);
                    if state.focus == PanelFocus::Conversation {
                        state.conversation_follow = false;
                    }
                }
                KeyCode::Down => scroll_active_panel(&mut state, 1),
                KeyCode::PageUp => {
                    scroll_active_panel(&mut state, -8);
                    if state.focus == PanelFocus::Conversation {
                        state.conversation_follow = false;
                    }
                }
                KeyCode::PageDown => scroll_active_panel(&mut state, 8),
                KeyCode::Home => {
                    set_active_scroll(&mut state, 0);
                    if state.focus == PanelFocus::Conversation {
                        state.conversation_follow = false;
                    }
                }
                KeyCode::End => {
                    set_active_scroll(&mut state, u16::MAX);
                    if state.focus == PanelFocus::Conversation {
                        state.conversation_follow = true;
                        state.conversation_unseen = 0;
                        state.conversation_needs_autoscroll = false;
                    }
                }
                KeyCode::Enter => {
                    if key_event.ctrl {
                        input_insert_char(&mut state, '\n');
                        return;
                    }
                    if state.pending {
                        return;
                    }

                    let submitted = state.input.clone();
                    input_clear(&mut state);
                    if submitted.trim().is_empty() {
                        return;
                    }
                    if submitted.trim() == "/model" {
                        drop(state);
                        model_selector.borrow_mut().visible = true;
                        model_selector.borrow_mut().loading = true;
                        model_selector.borrow_mut().selected = 0;
                        model_selector.borrow_mut().filter.clear();
                        model_selector.borrow_mut().items.clear();
                        let model_selector = model_selector.clone();
                        let app = app.clone();
                        spawn_local(async move {
                            match fetch_models_catalog().await {
                                Ok(catalog) => {
                                    let mut selector = model_selector.borrow_mut();
                                    let mut items = Vec::new();
                                    for provider in catalog.providers {
                                        for model in provider.models {
                                            let label = model
                                                .name
                                                .clone()
                                                .unwrap_or_else(|| model.id.clone());
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
                                    selector.selected = items
                                        .iter()
                                        .position(|m| {
                                            m.provider == catalog.current_provider
                                                && m.model == catalog.current_model
                                        })
                                        .unwrap_or(0);
                                    selector.filter.clear();
                                    selector.current_provider = catalog.current_provider;
                                    selector.current_model = catalog.current_model;
                                    selector.items = items;
                                    selector.loading = false;
                                }
                                Err(err) => {
                                    let mut selector = model_selector.borrow_mut();
                                    selector.visible = false;
                                    selector.loading = false;
                                    selector.items.clear();
                                    register_error(&mut app.borrow_mut(), &err);
                                }
                            }
                        });
                        return;
                    }
                    if submitted.trim() == "/session" {
                        drop(state);
                        session_dialog.borrow_mut().visible = true;
                        session_dialog.borrow_mut().loading = true;
                        session_dialog.borrow_mut().selected = 0;
                        session_dialog.borrow_mut().filter.clear();
                        session_dialog.borrow_mut().items.clear();
                        session_dialog.borrow_mut().current_session_id =
                            app.borrow().snapshot.session.id.clone();
                        let session_dialog = session_dialog.clone();
                        let app = app.clone();
                        spawn_local(async move {
                            match fetch_sessions().await {
                                Ok(items) => {
                                    let mut dialog = session_dialog.borrow_mut();
                                    dialog.visible = true;
                                    dialog.loading = false;
                                    dialog.items = items;
                                }
                                Err(err) => {
                                    let mut dialog = session_dialog.borrow_mut();
                                    dialog.visible = false;
                                    dialog.loading = false;
                                    dialog.items.clear();
                                    register_error(&mut app.borrow_mut(), &err);
                                }
                            }
                        });
                        return;
                    }
                    if submitted.trim() == "/help" || submitted.trim() == "/h" {
                        drop(state);
                        *help_dialog_visible.borrow_mut() = true;
                        return;
                    }
                    if submitted.trim() == "/mcp" {
                        drop(state);
                        mcp_dialog.borrow_mut().visible = true;
                        mcp_dialog.borrow_mut().loading = true;
                        mcp_dialog.borrow_mut().lines.clear();
                        let mcp_dialog = mcp_dialog.clone();
                        let app = app.clone();
                        spawn_local(async move {
                            match fetch_mcp_status().await {
                                Ok(snapshot) => {
                                    let mut dialog = mcp_dialog.borrow_mut();
                                    dialog.runtime = snapshot.runtime;
                                    dialog.lines = snapshot
                                        .servers
                                        .into_iter()
                                        .map(|s| {
                                            let mut line = format!(
                                                "{} enabled={} state={}",
                                                s.name, s.enabled, s.state
                                            );
                                            if let Some(count) = s.tool_count {
                                                line.push_str(&format!(" tools={count}"));
                                            }
                                            if let Some(err) = s.error {
                                                line.push_str(&format!(" error={err}"));
                                            }
                                            line
                                        })
                                        .collect();
                                    dialog.loading = false;
                                }
                                Err(err) => {
                                    let mut dialog = mcp_dialog.borrow_mut();
                                    dialog.visible = false;
                                    dialog.loading = false;
                                    dialog.lines.clear();
                                    register_error(&mut app.borrow_mut(), &err);
                                }
                            }
                        });
                        return;
                    }

                    state.pending = true;
                    state.live_output.clear();
                    register_submit(&mut state, &submitted);

                    drop(state);
                    let app = app.clone();
                    spawn_local(async move {
                        if let Err(err) = start_message(&submitted).await {
                            let mut state = app.borrow_mut();
                            state.pending = false;
                            state.live_output.clear();
                            register_error(&mut state, &err);
                        };
                    });
                }
                _ => {}
            }
        }
    });

    let mut tick: u64 = 0;
    terminal.draw_web(move |f| {
        tick = tick.wrapping_add(1);
        let mut state = app.borrow_mut();
        let selector = model_selector.borrow();
        let session_dialog = session_dialog.borrow();
        let dialog = mcp_dialog.borrow();
        let help_visible = *help_dialog_visible.borrow();

        let root = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(3),
                Constraint::Length(5),
            ])
            .split(f.area());

        let conv = conversation_render_plain(&state, &state.live_output);
        let conv_view_h = root[1].height.saturating_sub(2);
        let conv_wrap_w = root[1].width.saturating_sub(2);
        let conv_max = content_max_scroll_wrapped(&conv, conv_view_h, conv_wrap_w);
        if state.conversation_follow || state.conversation_needs_autoscroll {
            state.conversation_scroll = conv_max;
            state.conversation_follow = true;
            state.conversation_unseen = 0;
            state.conversation_needs_autoscroll = false;
        } else {
            state.conversation_scroll = state.conversation_scroll.min(conv_max);
            update_conversation_follow_state(&mut state, conv_max);
        }
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

        f.render_widget(
            Paragraph::new(status_line(&state.snapshot))
                .style(Style::default().fg(Color::White))
                .block(
                    Block::default()
                        .title("zAgent Web")
                        .borders(Borders::ALL)
                        .title_style(
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        )
                        .border_style(Style::default().fg(Color::Gray)),
                ),
            root[0],
        );

        let conv_scroll = state.conversation_scroll.min(conv_max);
        let conv_title = if state.conversation_unseen > 0 {
            format!("Conversation (+{} new)", state.conversation_unseen)
        } else {
            "Conversation".to_string()
        };
        f.render_widget(
            Paragraph::new(render_conversation_text(
                &state.lines,
                state.show_tool_result_details,
                state.pending,
                &state.live_output,
            ))
            .block(panel_block(
                panel_title(&conv_title, state.focus == PanelFocus::Conversation),
                state.focus == PanelFocus::Conversation,
            ))
            .scroll((conv_scroll, 0))
            .wrap(Wrap { trim: false }),
            root[1],
        );
        if conv_max > 0 {
            let conv_inner = root[1].inner(ratzilla::ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            let mut conv_scroll_state = ScrollbarState::new(wrapped_line_count(&conv, conv_wrap_w))
                .viewport_content_length(conv_view_h as usize)
                .position(conv_scroll as usize);
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

        let feedback_raw = if state.pending {
            let spinner = ["|", "/", "-", "\\"][(tick as usize / 4) % 4];
            if state.live_output.is_empty() {
                format!("{spinner} model thinking...")
            } else {
                format!(
                    "{spinner} model output: {}",
                    tail_single_line(&state.live_output, 90)
                )
            }
        } else {
            latest_feedback(&state)
        };
        let feedback_head = scroll_line(
            &feedback_raw,
            tick / 2,
            root[2].width.saturating_sub(2) as usize,
        );
        let feedback_text = format!("{feedback_head}\n{}", activity_text(&state.activity));
        let feedback_scroll = state.activity_scroll.min(content_max_scroll(
            &feedback_text,
            root[2].height.saturating_sub(2),
        ));
        f.render_widget(
            Paragraph::new(feedback_text)
                .style(Style::default().fg(Color::LightBlue))
                .block(panel_block(
                    panel_title("Feedback", state.focus == PanelFocus::Activity),
                    state.focus == PanelFocus::Activity,
                ))
                .scroll((feedback_scroll, 0)),
            root[2],
        );

        f.render_widget(
            Paragraph::new(format!(
                "{}\n{}",
                prompt_text(&state.input, state.input_cursor, true),
                usage_metrics_line(&state.snapshot)
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
            ),
            root[3],
        );

        if selector.visible {
            let popup = centered_rect(80, 70, f.area());
            f.render_widget(
                Block::default()
                    .title("Model Selector")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
                popup,
            );
            let inner = popup.inner(ratzilla::ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            let text = if selector.loading {
                "Loading models...".to_string()
            } else if selector.items.is_empty() {
                "No models returned by configured providers.".to_string()
            } else {
                let filtered = filtered_model_indices(&selector);
                let selected = selector.selected.min(filtered.len().saturating_sub(1));
                let mut out = String::new();
                out.push_str(&format!(
                    "Filter: {}\nCurrent: {} / {}\n\n",
                    selector.filter, selector.current_provider, selector.current_model
                ));
                if filtered.is_empty() {
                    out.push_str("No matches.\n");
                    out.push_str("(Type to filter, Delete clears filter)\n");
                } else {
                    for (pos, idx) in filtered.iter().copied().enumerate() {
                        let item = &selector.items[idx];
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
            let inner = popup.inner(ratzilla::ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            f.render_widget(
                Paragraph::new(session_dialog_text(&session_dialog))
                    .style(Style::default().fg(Color::White))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }

        if dialog.visible {
            let popup = centered_rect(80, 60, f.area());
            f.render_widget(
                Block::default()
                    .title("MCP Servers")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
                popup,
            );
            let inner = popup.inner(ratzilla::ratatui::layout::Margin {
                vertical: 1,
                horizontal: 1,
            });
            let text = if dialog.loading {
                "Loading MCP server status...".to_string()
            } else if dialog.lines.is_empty() {
                format!("runtime={}\nNo MCP servers configured.", dialog.runtime)
            } else {
                format!(
                    "runtime={}\n\n{}\n\n(Press Esc or Enter to close)",
                    dialog.runtime,
                    dialog.lines.join("\n")
                )
            };
            f.render_widget(
                Paragraph::new(text)
                    .style(Style::default().fg(Color::White))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }

        if help_visible {
            let popup = centered_rect(80, 70, f.area());
            f.render_widget(
                Block::default()
                    .title("Help")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
                popup,
            );
            let inner = popup.inner(ratzilla::ratatui::layout::Margin {
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
    });

    Ok(())
}

fn base_url() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8787".to_string())
}

async fn fetch_state() -> Result<BackendSnapshot, String> {
    let resp = Request::get(&format!("{}/api/state", base_url()))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<BackendSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn fetch_models_catalog() -> Result<ModelCatalogSnapshot, String> {
    let resp = Request::get(&format!("{}/api/models", base_url()))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<ModelCatalogSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn fetch_mcp_status() -> Result<McpStatusSnapshot, String> {
    let resp = Request::get(&format!("{}/api/mcp", base_url()))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<McpStatusSnapshot>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn fetch_sessions() -> Result<Vec<SessionListItem>, String> {
    let resp = Request::get(&format!("{}/api/sessions", base_url()))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<SessionsApiResponse>()
        .await
        .map(|payload| payload.sessions)
        .map_err(|e| format!("invalid response: {e}"))
}

async fn create_session(name: Option<String>) -> Result<BackendMessageResponse, String> {
    let body = NewSessionRequest { name };
    let resp = Request::post(&format!("{}/api/session/new", base_url()))
        .json(&body)
        .map_err(|e| format!("request build failed: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<BackendMessageResponse>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn continue_session(name_or_id: &str) -> Result<BackendMessageResponse, String> {
    let body = ContinueSessionRequest {
        name_or_id: name_or_id.to_string(),
    };
    let resp = Request::post(&format!("{}/api/session/continue", base_url()))
        .json(&body)
        .map_err(|e| format!("request build failed: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<BackendMessageResponse>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn set_model_selection(
    provider: &str,
    model: &str,
) -> Result<BackendMessageResponse, String> {
    let body = SetModelRequest {
        provider: provider.to_string(),
        model: model.to_string(),
    };
    let resp = Request::post(&format!("{}/api/model", base_url()))
        .json(&body)
        .map_err(|e| format!("request build failed: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    resp.json::<BackendMessageResponse>()
        .await
        .map_err(|e| format!("invalid response: {e}"))
}

async fn start_message(input: &str) -> Result<(), String> {
    let body = MessageRequest {
        input: input.to_string(),
    };
    Request::post(&format!("{}/api/message/start", base_url()))
        .json(&body)
        .map_err(|e| format!("request build failed: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    Ok(())
}

fn start_events_stream(app: Rc<RefCell<UiState>>) {
    let url = format!("{}/api/events/stream", base_url());
    let event_source = match web_sys::EventSource::new(&url) {
        Ok(es) => es,
        Err(err) => {
            register_error(
                &mut app.borrow_mut(),
                &format!("failed to open EventSource: {:?}", err),
            );
            return;
        }
    };

    let es_for_message = event_source.clone();
    let app_for_message = app.clone();
    let on_message = Closure::<dyn FnMut(_)>::new(move |event: web_sys::MessageEvent| {
        let payload = event.data().as_string().unwrap_or_default();
        let parsed: Result<StreamChunk, _> = serde_json::from_str(&payload);
        let mut state = app_for_message.borrow_mut();
        match parsed {
            Ok(chunk) => {
                let mut live = std::mem::take(&mut state.live_output);
                apply_stream_chunk(&mut state, &mut live, &chunk);
                state.live_output = live;
            }
            Err(err) => {
                state.pending = false;
                state.live_output.clear();
                register_error(&mut state, &format!("stream parse error: {err}"));
                es_for_message.close();
            }
        }
    });
    event_source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let app_for_error = app.clone();
    let on_error = Closure::<dyn FnMut(_)>::new(move |_event: web_sys::Event| {
        let mut state = app_for_error.borrow_mut();
        if state.pending {
            state.pending = false;
            state.live_output.clear();
            register_error(&mut state, "events stream connection error");
        } else {
            set_feedback(
                &mut state,
                "events stream disconnected (reload page to reconnect)",
            );
        }
    });
    event_source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();
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

fn set_active_scroll(ui: &mut UiState, value: u16) {
    match ui.focus {
        PanelFocus::Conversation => ui.conversation_scroll = value,
        PanelFocus::Activity => ui.activity_scroll = value,
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

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: ratzilla::ratatui::layout::Rect,
) -> ratzilla::ratatui::layout::Rect {
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
