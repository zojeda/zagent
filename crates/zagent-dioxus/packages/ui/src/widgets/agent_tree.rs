use dioxus::prelude::*;

use crate::models::AgentNodeView;
use crate::widgets::TerminalStream;

#[derive(Props, Clone, PartialEq)]
pub struct AgentTreeProps {
    pub roots: Vec<AgentNodeView>,
    pub on_open_model_event: EventHandler<String>,
}

#[component]
pub fn AgentTree(props: AgentTreeProps) -> Element {
    rsx! {
        div { class: "cards-root",
            for node in props.roots {
                AgentCard {
                    node,
                    on_open_model_event: props.on_open_model_event
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct AgentCardProps {
    node: AgentNodeView,
    on_open_model_event: EventHandler<String>,
}

#[component]
fn AgentCard(props: AgentCardProps) -> Element {
    let mut expanded = use_signal(|| true);
    let node = props.node;
    let badge_class = format!("phase-badge {}", badge_class(&node.phase_badge));
    let caret_class = if expanded() {
        "agent-caret open"
    } else {
        "agent-caret"
    };
    rsx! {
        article { class: "agent-card",
            style: format!("--depth:{}", node.depth),
            button {
                class: "agent-header",
                onclick: move |_| expanded.set(!expanded()),
                span { class: caret_class, "▾" }
                div { class: "agent-header-main",
                    div { class: "agent-title",
                        h3 { class: "agent-name", "{node.name}" }
                        span { class: badge_class, "{node.phase_badge}" }
                    }
                    span { class: "agent-depth", "depth {node.depth}" }
                }
            }
            if expanded() {
                div { class: "agent-content",
                    div { class: "metric-grid",
                        MetricPill { label: "sent", value: node.tokens_sent.to_string() }
                        MetricPill { label: "received", value: node.tokens_received.to_string() }
                        MetricPill { label: "cache", value: node.cached_tokens.to_string() }
                        MetricPill { label: "context", value: format!("{:.1}%", node.context_pct) }
                        MetricPill { label: "cost", value: format!("${:.6}", node.cost_usd) }
                    }
                    if let Some(text) = node.reasoning_text {
                        p { class: "reasoning-line", em { "{text}" } }
                    }
                    if node.handoff_message.is_some() || node.handoff_context.is_some() {
                        div { class: "handoff-block",
                            if let Some(msg) = node.handoff_message {
                                p { class: "handoff-line", strong { "Handoff message: " } "{msg}" }
                            }
                            if let Some(ctx) = node.handoff_context {
                                p { class: "handoff-line", strong { "Handoff context: " } "{ctx}" }
                            }
                        }
                    }
                    p { class: "last-event", "{node.last_event}" }
                    if !node.timeline.is_empty() {
                        details { class: "timeline", open: true,
                            summary { "Event timeline ({node.timeline.len()})" }
                            div { class: "timeline-list",
                                for entry in node.timeline {
                                    TimelineEntryCard {
                                        entry,
                                        on_open_model_event: props.on_open_model_event.clone()
                                    }
                                }
                            }
                        }
                    }
                    if !node.children.is_empty() {
                        div { class: "child-cards",
                            for child in node.children {
                                AgentCard {
                                    node: child,
                                    on_open_model_event: props.on_open_model_event
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct TimelineEntryCardProps {
    entry: crate::models::AgentTimelineEntryView,
    on_open_model_event: EventHandler<String>,
}

#[component]
fn TimelineEntryCard(props: TimelineEntryCardProps) -> Element {
    let entry = props.entry;
    let mut open = use_signal(|| entry.running);
    let mut last_running = use_signal(|| entry.running);

    use_effect(move || {
        if last_running() != entry.running {
            open.set(entry.running);
            last_running.set(entry.running);
        }
    });

    let can_expand = entry.response_text.is_some()
        || (entry.show_terminal && !entry.terminal_segments.is_empty())
        || entry.running;

    rsx! {
        article { class: "timeline-entry-card timeline-entry-card-{entry.kind}",
            div { class: "timeline-entry-header",
                div { class: "timeline-entry-headline",
                    span { class: "timeline-entry-title", "{entry.title}" }
                    if let Some(tool_call_id) = entry.tool_call_id.clone() {
                        span { class: "timeline-entry-id", "#{tool_call_id}" }
                    }
                    if entry.running {
                        span { class: "timeline-entry-live", "live" }
                    }
                }
                div { class: "timeline-entry-actions",
                    if let Some(event_id) = entry.request_event_id.clone() {
                        button {
                            class: "model-json-action-button",
                            onclick: {
                                let on_open = props.on_open_model_event.clone();
                                move |_| on_open.call(event_id.clone())
                            },
                            "Request"
                        }
                    }
                    if let Some(event_id) = entry.response_event_id.clone() {
                        button {
                            class: "model-json-action-button",
                            onclick: {
                                let on_open = props.on_open_model_event.clone();
                                move |_| on_open.call(event_id.clone())
                            },
                            "Response"
                        }
                    }
                    if can_expand {
                        button {
                            class: if open() { "timeline-expand-toggle open" } else { "timeline-expand-toggle" },
                            onclick: move |_| open.set(!open()),
                            if open() { "Hide" } else { "Show" }
                        }
                    }
                }
            }
            p { class: "timeline-entry-request", "{entry.request_text}" }
            if open() {
                div { class: "timeline-entry-body",
                    if let Some(response_text) = entry.response_text.clone() {
                        p { class: "timeline-entry-response", "{response_text}" }
                    }
                    if entry.show_terminal && (!entry.terminal_segments.is_empty() || entry.running) {
                        TerminalStream {
                            key: "terminal-{entry.tool_call_id.clone().unwrap_or_default()}",
                            tool_call_id: entry.tool_call_id.clone(),
                            running: entry.running,
                            segments: entry.terminal_segments.clone()
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct MetricPillProps {
    label: &'static str,
    value: String,
}

#[component]
fn MetricPill(props: MetricPillProps) -> Element {
    rsx! {
        div { class: "metric-pill",
            span { class: "metric-label", "{props.label}" }
            span { class: "metric-value", "{props.value}" }
        }
    }
}

fn badge_class(phase_badge: &str) -> &'static str {
    if phase_badge.contains("active") {
        "badge-active"
    } else if phase_badge.contains("done") {
        "badge-done"
    } else if phase_badge.contains("tool") {
        "badge-tool"
    } else {
        "badge-idle"
    }
}
