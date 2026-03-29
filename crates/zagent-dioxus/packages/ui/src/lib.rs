//! Shared Dioxus UI for zAgent clients.

mod app;
mod models;
mod styles;
mod widgets;

pub use app::Dashboard;
pub use models::{
    AgentNodeView, AgentTimelineEntryView, ChatTurnView, FooterTotalsView, PromptImagePreviewView,
    ServerTabView, TerminalSegmentView,
};
pub use styles::UiWidgetStyles;
pub use widgets::{AgentTree, ChatTimeline, FooterTotals, MarkdownCard, PromptInput, ServerTabs};
