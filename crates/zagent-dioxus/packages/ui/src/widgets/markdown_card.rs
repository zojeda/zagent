use dioxus::prelude::*;
use pulldown_cmark::{html, Options, Parser};

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

/// Converts markdown to HTML, with special handling for mermaid code blocks.
/// Mermaid blocks (```mermaid) are converted to `<pre class="mermaid">` tags
/// so that the mermaid.js library can render them as diagrams.
fn markdown_to_html(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    // First, generate standard HTML from markdown
    let parser = Parser::new_ext(input, options);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);

    // Post-process to transform mermaid code blocks
    // pulldown-cmark outputs: <pre><code class="language-mermaid">diagram content</code></pre>
    // We want: <pre class="mermaid">diagram content</pre>
    transform_mermaid_blocks(&html_out)
}

/// Transforms standard mermaid code block HTML to mermaid-specific format.
///
/// Input:  `<pre><code class="language-mermaid">graph TD; A-->B;</code></pre>`
/// Output: `<pre class="mermaid">graph TD; A-->B;</pre>`
fn transform_mermaid_blocks(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for <pre><code class="language-mermaid">
        if i + 35 < len {
            // Check for the pattern: <pre><code class="language-mermaid">
            let slice = &bytes[i..i + 35];
            let slice_str = String::from_utf8_lossy(slice);

            if slice_str == "<pre><code class=\"language-mermaid\">" {
                // Replace with <pre class="mermaid">
                result.push_str("<pre class=\"mermaid\">");
                i += 35;

                // Find the closing </code></pre> and replace with just </pre>
                // Search for </code></pre>
                let remaining = &bytes[i..];
                if let Some(end_pos) = find_mermaid_end(remaining) {
                    // Add the mermaid content (everything between the opening and </code>)
                    result.push_str(&String::from_utf8_lossy(&remaining[..end_pos]));
                    // Add closing </pre> instead of </code></pre>
                    result.push_str("</pre>");
                    i += end_pos + 16; // Skip past </code></pre> (16 chars)
                    continue;
                }
            }
        }

        // No match, just copy the character
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

/// Finds the end of a mermaid block by looking for </code></pre>
/// Returns the position of '<' in </code></pre>
fn find_mermaid_end(remaining: &[u8]) -> Option<usize> {
    // Look for </code></pre>
    let end_marker = b"</code></pre>";
    let end_len = end_marker.len();

    for i in 0..remaining.len() - end_len + 1 {
        if &remaining[i..i + end_len] == end_marker {
            return Some(i);
        }
    }
    None
}
