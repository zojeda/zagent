use dioxus::prelude::*;

use crate::widgets::markdown_render::markdown_to_html;

#[derive(Props, Clone, PartialEq)]
pub struct MarkdownCardProps {
    pub title: String,
    pub markdown: String,
    pub empty_label: String,
}

#[component]
pub fn MarkdownCard(props: MarkdownCardProps) -> Element {
    if props.markdown.trim().is_empty() {
        return rsx! {
            section { class: "markdown-card",
                h3 { class: "markdown-title", "{props.title}" }
                p { class: "markdown-empty", "{props.empty_label}" }
            }
        };
    }

    let rendered = markdown_to_html(&props.markdown);
    rsx! {
        section { class: "markdown-card",
            h3 { class: "markdown-title", "{props.title}" }
            article { class: "markdown-body", dangerous_inner_html: "{rendered}" }
        }
    }
}
