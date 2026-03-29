use dioxus::prelude::*;

use crate::models::ChatTurnView;
use crate::widgets::markdown_render::markdown_to_html;
use crate::widgets::AgentTree;

#[derive(Props, Clone, PartialEq)]
pub struct ChatTimelineProps {
    pub turns: Vec<ChatTurnView>,
    pub on_toggle_turn_details: EventHandler<usize>,
    pub on_open_model_event: EventHandler<String>,
}

#[component]
pub fn ChatTimeline(props: ChatTimelineProps) -> Element {
    if props.turns.is_empty() {
        return rsx! {};
    }

    rsx! {
        section { class: "chat-timeline",
            for turn in props.turns {
                article {
                    key: "chat-turn-{turn.id}",
                    class: "chat-turn chat-turn-{turn.role_class}",
                    header { class: "chat-turn-header",
                        h3 { class: "chat-turn-title", "{turn.author}" }
                    }
                    div {
                        class: "markdown-body chat-turn-body",
                        dangerous_inner_html: "{markdown_to_html(&turn.markdown)}"
                    }
                    if turn.markdown.trim().is_empty() {
                        p { class: "chat-turn-placeholder", "thinking..." }
                    }
                    if !turn.agent_roots.is_empty() {
                        div { class: "chat-turn-details",
                            button {
                                class: if turn.agent_details_collapsed {
                                    "chat-turn-details-toggle collapsed"
                                } else {
                                    "chat-turn-details-toggle"
                                },
                                onclick: {
                                    let on_toggle = props.on_toggle_turn_details.clone();
                                    move |_| on_toggle.call(turn.id)
                                },
                                if turn.agent_details_collapsed {
                                    "Show agent run details"
                                } else {
                                    "Hide agent run details"
                                }
                            }
                            if !turn.agent_details_collapsed {
                                div {
                                    class: if turn.agent_details_enabled {
                                        "chat-turn-details-body"
                                    } else {
                                        "chat-turn-details-body disabled"
                                    },
                                    AgentTree {
                                        roots: turn.agent_roots.clone(),
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
}
