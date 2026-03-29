use pulldown_cmark::{html, Options, Parser};

/// Converts markdown to HTML, with special handling for mermaid code blocks.
/// Mermaid blocks (```mermaid) are converted to placeholder nodes that the web
/// app upgrades into rendered SVG diagrams after the markdown enters the DOM.
pub fn markdown_to_html(input: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(input, options);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);

    transform_mermaid_blocks(&html_out)
}

fn transform_mermaid_blocks(html: &str) -> String {
    const OPEN: &str = "<pre><code class=\"language-mermaid\">";
    const CLOSE: &str = "</code></pre>";

    let mut result = String::with_capacity(html.len());
    let mut remaining = html;

    while let Some(start) = remaining.find(OPEN) {
        let before = &remaining[..start];
        result.push_str(before);
        remaining = &remaining[start + OPEN.len()..];

        if let Some(end) = remaining.find(CLOSE) {
            result.push_str("<div class=\"mermaid-graph\">");
            result.push_str(&remaining[..end]);
            result.push_str("</div>");
            remaining = &remaining[end + CLOSE.len()..];
        } else {
            result.push_str(OPEN);
            result.push_str(remaining);
            return result;
        }
    }

    result.push_str(remaining);
    result
}

#[cfg(test)]
mod tests {
    use super::markdown_to_html;

    #[test]
    fn mermaid_blocks_become_render_placeholders() {
        let html = markdown_to_html("```mermaid\ngraph TD\nA-->B\n```");

        assert!(html.contains("<div class=\"mermaid-graph\">"));
        assert!(html.contains("graph TD"));
        assert!(html.contains("A--&gt;B"));
        assert!(!html.contains("language-mermaid"));
    }

    #[test]
    fn utf8_content_is_preserved_alongside_mermaid_blocks() {
        let html = markdown_to_html("Hello! 👋\n\n```mermaid\ngraph TD\nA-->B\n```");

        assert!(html.contains("Hello! 👋"));
        assert!(!html.contains("ð"));
    }
}
