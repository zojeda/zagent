use dioxus::prelude::*;

const DASHBOARD_CSS: Asset = asset!("/assets/styling/widgets/dashboard.css");
const SERVER_TABS_CSS: Asset = asset!("/assets/styling/widgets/server_tabs.css");
const AGENT_TREE_CSS: Asset = asset!("/assets/styling/widgets/agent_tree.css");
const FOOTER_TOTALS_CSS: Asset = asset!("/assets/styling/widgets/footer_totals.css");
const PROMPT_INPUT_CSS: Asset = asset!("/assets/styling/widgets/prompt_input.css");
const MARKDOWN_CARD_CSS: Asset = asset!("/assets/styling/widgets/markdown_card.css");
const CHAT_TIMELINE_CSS: Asset = asset!("/assets/styling/widgets/chat_timeline.css");
const TERMINAL_STREAM_CSS: Asset = asset!("/assets/styling/widgets/terminal_stream.css");

#[component]
pub fn UiWidgetStyles() -> Element {
    rsx! {
        document::Stylesheet { href: DASHBOARD_CSS }
        document::Stylesheet { href: SERVER_TABS_CSS }
        document::Stylesheet { href: AGENT_TREE_CSS }
        document::Stylesheet { href: FOOTER_TOTALS_CSS }
        document::Stylesheet { href: PROMPT_INPUT_CSS }
        document::Stylesheet { href: MARKDOWN_CARD_CSS }
        document::Stylesheet { href: CHAT_TIMELINE_CSS }
        document::Stylesheet { href: TERMINAL_STREAM_CSS }
    }
}
