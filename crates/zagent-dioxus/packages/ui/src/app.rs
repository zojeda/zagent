use dioxus::prelude::*;

use crate::models::{ChatTurnView, FooterTotalsView, ServerTabView};
use crate::styles::UiWidgetStyles;
use crate::widgets::{ChatTimeline, FooterTotals, PromptInput, ServerTabs};

#[derive(Props, Clone, PartialEq)]
pub struct DashboardProps {
    pub title: String,
    pub status_text: String,
    pub connected: bool,
    pub pending: bool,
    pub prompt_value: String,
    pub turns: Vec<ChatTurnView>,
    pub totals: FooterTotalsView,
    pub tabs: Vec<ServerTabView>,
    pub active_tab_id: usize,
    pub show_connect_form: bool,
    pub connect_value: String,
    pub on_select_tab: EventHandler<usize>,
    pub on_close_tab: EventHandler<usize>,
    pub on_reconnect_tab: EventHandler<usize>,
    pub on_toggle_add_server: EventHandler<()>,
    pub on_connect_input: EventHandler<String>,
    pub on_connect_submit: EventHandler<()>,
    pub on_prompt_input: EventHandler<String>,
    pub on_submit: EventHandler<()>,
    pub on_toggle_turn_details: EventHandler<usize>,
    pub on_open_model_event: EventHandler<String>,
    pub on_conversation_scroll: EventHandler<()>,
    pub show_resume_autoscroll: bool,
    pub resume_autoscroll_label: String,
    pub on_resume_autoscroll: EventHandler<()>,
}

#[component]
pub fn Dashboard(props: DashboardProps) -> Element {
    rsx! {
        main { class: "app-shell",
            UiWidgetStyles {}
            section { class: "status-bar",
                h1 { class: "title", "{props.title}" }
                div { class: "status-group",
                    span {
                        class: if props.connected { "conn conn-live" } else { "conn conn-down" },
                        if props.connected { "live" } else { "offline" }
                    }
                    span { class: "status-text", "{props.status_text}" }
                }
            }

            section { class: "tabs-bar",
                ServerTabs {
                    tabs: props.tabs,
                    active_tab_id: props.active_tab_id,
                    on_select: props.on_select_tab,
                    on_close: props.on_close_tab,
                    on_reconnect: props.on_reconnect_tab,
                    on_add: props.on_toggle_add_server
                }
                if props.show_connect_form {
                    div { class: "connect-shell",
                        input {
                            class: "connect-input",
                            value: "{props.connect_value}",
                            placeholder: "https://your-zagent-server",
                            oninput: {
                                let on_connect_input = props.on_connect_input.clone();
                                move |evt| on_connect_input.call(evt.value())
                            },
                            onkeydown: {
                                let on_connect_submit = props.on_connect_submit.clone();
                                move |evt| {
                                    if evt.key() == Key::Enter {
                                        on_connect_submit.call(());
                                    }
                                }
                            }
                        }
                        button {
                            class: "connect-button",
                            onclick: move |_| props.on_connect_submit.call(()),
                            "Connect"
                        }
                    }
                }
            }

            section {
                id: "conversation-scroll",
                class: "conversation-pane",
                onscroll: move |_| props.on_conversation_scroll.call(()),
                ChatTimeline {
                    turns: props.turns,
                    on_toggle_turn_details: props.on_toggle_turn_details,
                    on_open_model_event: props.on_open_model_event
                }
            }

            footer { class: "footer-pane",
                FooterTotals { totals: props.totals }
                if props.show_resume_autoscroll {
                    button {
                        class: "resume-autoscroll-button",
                        onclick: move |_| props.on_resume_autoscroll.call(()),
                        "{props.resume_autoscroll_label}"
                    }
                }
                PromptInput {
                    value: props.prompt_value,
                    pending: props.pending,
                    on_input: props.on_prompt_input,
                    on_submit: props.on_submit,
                    div { class: "prompt-hint", "Shift future tools here" }
                }
            }
        }
    }
}
