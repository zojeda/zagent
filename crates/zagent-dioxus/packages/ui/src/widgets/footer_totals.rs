use dioxus::prelude::*;

use crate::models::FooterTotalsView;

#[derive(Props, Clone, PartialEq)]
pub struct FooterTotalsProps {
    pub totals: FooterTotalsView,
}

#[component]
pub fn FooterTotals(props: FooterTotalsProps) -> Element {
    let totals = &props.totals;
    rsx! {
        div { class: "totals-row",
            span { class: "totals-label", "Top agent: {totals.top_agent}" }
            MetricPill { label: "sent", value: totals.tokens_sent.to_string() }
            MetricPill { label: "received", value: totals.tokens_received.to_string() }
            MetricPill { label: "cache", value: totals.cached_tokens.to_string() }
            MetricPill { label: "context", value: format!("{:.1}%", totals.context_pct) }
            MetricPill { label: "cost", value: format!("${:.6}", totals.cost_usd) }
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
