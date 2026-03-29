use dioxus::prelude::*;

use crate::models::TerminalSegmentView;

#[derive(Props, Clone, PartialEq)]
pub struct TerminalStreamProps {
    pub tool_call_id: Option<String>,
    pub running: bool,
    pub segments: Vec<TerminalSegmentView>,
}

#[component]
pub fn TerminalStream(props: TerminalStreamProps) -> Element {
    let mut open = use_signal(|| props.running);
    let mut last_running = use_signal(|| props.running);

    use_effect(move || {
        if last_running() != props.running {
            open.set(props.running);
            last_running.set(props.running);
        }
    });

    let label = props
        .tool_call_id
        .clone()
        .map(|id| format!("Shell output #{id}"))
        .unwrap_or_else(|| "Shell output".to_string());

    rsx! {
        div {
            class: "timeline-terminal-shell",
            button {
                class: "timeline-terminal-summary",
                onclick: move |_| open.set(!open()),
                "{label}"
                if props.running {
                    span { class: "timeline-terminal-live-badge", "running" }
                }
            }
            if open() {
                div { class: "timeline-terminal",
                    for segment in props.segments {
                        pre {
                            class: "timeline-terminal-segment timeline-terminal-{segment.channel}",
                            "{segment.text}"
                        }
                    }
                    if props.running {
                        div { class: "timeline-terminal-status", "stream open..." }
                    }
                }
            }
        }
    }
}
