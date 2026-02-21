use async_trait::async_trait;
use serde_json::Value;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// Fetch and return web page or API content by URL.
pub struct WebFetchTool;

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "webfetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL (web page or API) and return a trimmed text response."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute URL to fetch (http:// or https://)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum response characters to return (default 4000, max 20000)"
                },
                "include_headers": {
                    "type": "boolean",
                    "description": "Include status and content-type metadata in the output (default true)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("webfetch", "Missing 'url' parameter"))?;
        let max_chars = args
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .unwrap_or(4000)
            .clamp(200, 20_000) as usize;
        let include_headers = args
            .get("include_headers")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(zagent_core::Error::tool(
                "webfetch",
                "URL must start with http:// or https://",
            ));
        }

        let resp = reqwest::get(url).await.map_err(|e| {
            zagent_core::Error::tool("webfetch", format!("Fetch request failed: {e}"))
        })?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let body = resp.text().await.map_err(|e| {
            zagent_core::Error::tool("webfetch", format!("Failed to read response body: {e}"))
        })?;

        let cleaned = if content_type.contains("text/html") {
            html_to_text(&body)
        } else {
            body
        };
        let trimmed = truncate_chars(&cleaned, max_chars);

        let mut out = String::new();
        if include_headers {
            out.push_str(&format!("URL: {url}\n"));
            out.push_str(&format!("Status: {status}\n"));
            out.push_str(&format!("Content-Type: {content_type}\n"));
            out.push_str("----------------------------------------\n");
        }
        out.push_str(trimmed.trim());
        Ok(out)
    }
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let truncated: String = input.chars().take(max_chars).collect();
    format!("{truncated}\n\n[truncated to {max_chars} chars]")
}

fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut prev_space = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            c if c.is_whitespace() => {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            }
            c => {
                out.push(c);
                prev_space = false;
            }
        }
    }

    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}
