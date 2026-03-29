use dioxus::prelude::*;
use ui::{Dashboard, FooterTotalsView, PromptImagePreviewView};

const MAIN_CSS: Asset = asset!("/assets/main.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        Dashboard {
            title: "zAgent Desktop".to_string(),
            status_text: "Desktop shell ready".to_string(),
            connected: false,
            pending: false,
            recording: false,
            transcribing: false,
            prompt_value: String::new(),
            prompt_images: Vec::<PromptImagePreviewView>::new(),
            turns: Vec::new(),
            totals: FooterTotalsView {
                top_agent: "none".to_string(),
                ..FooterTotalsView::default()
            },
            tabs: Vec::new(),
            active_tab_id: 0,
            show_connect_form: false,
            connect_value: String::new(),
            on_select_tab: move |_| {},
            on_close_tab: move |_| {},
            on_reconnect_tab: move |_| {},
            on_toggle_add_server: move |_| {},
            on_connect_input: move |_| {},
            on_connect_submit: move |_| {},
            on_prompt_input: move |_| {},
            on_submit: move |_| {},
            on_copy_prompt: None,
            on_paste_prompt: None,
            on_prompt_paste: None,
            on_pick_images: None,
            on_remove_prompt_image: move |_| {},
            on_toggle_recording: None,
            on_toggle_turn_details: move |_| {},
            on_open_model_event: move |_| {},
            on_conversation_scroll: move |_| {},
            show_resume_autoscroll: false,
            resume_autoscroll_label: String::new(),
            on_resume_autoscroll: move |_| {}
        }
    }
}
