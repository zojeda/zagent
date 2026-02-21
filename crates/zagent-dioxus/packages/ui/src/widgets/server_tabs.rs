use dioxus::prelude::*;

use crate::models::ServerTabView;

#[derive(Props, Clone, PartialEq)]
pub struct ServerTabsProps {
    pub tabs: Vec<ServerTabView>,
    pub active_tab_id: usize,
    pub on_select: EventHandler<usize>,
    pub on_close: EventHandler<usize>,
    pub on_reconnect: EventHandler<usize>,
    pub on_add: EventHandler<()>,
}

#[component]
pub fn ServerTabs(props: ServerTabsProps) -> Element {
    let can_close = props.tabs.len() > 1;
    let tabs = props.tabs.clone();
    let reconnect_target = props
        .tabs
        .iter()
        .find(|t| t.id == props.active_tab_id)
        .and_then(|t| if t.connected { None } else { Some(t.id) });

    rsx! {
        div { class: "tabs-shell",
            for tab in tabs {
                button {
                    class: if tab.id == props.active_tab_id { "tab-btn tab-active" } else { "tab-btn" },
                    onclick: {
                        let on_select = props.on_select.clone();
                        let id = tab.id;
                        move |_| on_select.call(id)
                    },
                    span {
                        class: if tab.connected { "tab-dot dot-live" } else { "tab-dot dot-down" }
                    }
                    "{tab.label}"
                    if can_close {
                        span {
                            class: "tab-close",
                            onclick: {
                                let on_close = props.on_close.clone();
                                let id = tab.id;
                                move |evt| {
                                    evt.stop_propagation();
                                    on_close.call(id);
                                }
                            },
                            "x"
                        }
                    }
                }
            }
            if let Some(id) = reconnect_target {
                button {
                    class: "tab-btn tab-reconnect",
                    onclick: {
                        let on_reconnect = props.on_reconnect.clone();
                        move |_| on_reconnect.call(id)
                    },
                    "Reconnect"
                }
            }
            button {
                class: "tab-btn tab-add",
                onclick: move |_| props.on_add.call(()),
                "+"
            }
        }
    }
}
