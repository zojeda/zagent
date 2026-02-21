use dioxus::prelude::*;

use crate::models::AgentNodeView;

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
                        details { class: "timeline",
                            summary { "Event timeline ({node.timeline.len()})" }
                            ul {
                                for entry in node.timeline {
                                    li {
                                        class: "timeline-entry",
                                        span { class: "timeline-entry-text", "{entry.text}" }
                                        if (entry.kind == "model"
                                            && (entry.phase == "request_started"
                                                || entry.phase == "response_received"))
                                            || (entry.kind == "tool"
                                                && (entry.phase == "start"
                                                    || entry.phase == "finish"))
                                        {
                                            if let Some(event_id) = entry.event_id.clone() {
                                                button {
                                                    class: "model-json-action-button",
                                                    onclick: {
                                                        let on_open = props.on_open_model_event.clone();
                                                        move |_| on_open.call(event_id.clone())
                                                    },
                                                    "Details"
                                                }
                                            }
                                        }
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
