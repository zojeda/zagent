use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// Search the web using DuckDuckGo's public Instant Answer API.
pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct DdgResponse {
    #[serde(rename = "AbstractText")]
    abstract_text: String,
    #[serde(rename = "AbstractURL")]
    abstract_url: String,
    #[serde(rename = "Heading")]
    heading: String,
    #[serde(rename = "RelatedTopics")]
    related_topics: Vec<DdgTopic>,
}

#[derive(Debug, Deserialize)]
struct DdgTopic {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "FirstURL")]
    first_url: Option<String>,
    #[serde(rename = "Topics")]
    topics: Option<Vec<DdgTopic>>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        "Search the web for a query and return top results with URLs and snippets."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query text"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default 5, max 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("websearch", "Missing 'query' parameter"))?;
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 10) as usize;

        let mut url = reqwest::Url::parse("https://api.duckduckgo.com/").map_err(|e| {
            zagent_core::Error::tool("websearch", format!("Failed to build search URL: {e}"))
        })?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("format", "json")
            .append_pair("no_html", "1")
            .append_pair("skip_disambig", "1");

        let resp = reqwest::get(url).await.map_err(|e| {
            zagent_core::Error::tool("websearch", format!("Search request failed: {e}"))
        })?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            zagent_core::Error::tool("websearch", format!("Failed to read search response: {e}"))
        })?;
        if !status.is_success() {
            return Err(zagent_core::Error::tool(
                "websearch",
                format!("Search returned HTTP {status}"),
            ));
        }

        let parsed: DdgResponse = serde_json::from_str(&body).map_err(|e| {
            zagent_core::Error::tool("websearch", format!("Invalid search response JSON: {e}"))
        })?;

        let mut results = Vec::new();
        if !parsed.abstract_text.trim().is_empty() || !parsed.abstract_url.trim().is_empty() {
            let heading = if parsed.heading.trim().is_empty() {
                "Summary".to_string()
            } else {
                parsed.heading
            };
            results.push((heading, parsed.abstract_url, parsed.abstract_text));
        }
        collect_topics(&parsed.related_topics, &mut results);

        if results.is_empty() {
            return Ok(format!("No search results found for query: {query}"));
        }

        let mut out = String::new();
        out.push_str(&format!("Search results for: {query}\n"));
        out.push_str("----------------------------------------\n");
        for (idx, (title, url, snippet)) in results.into_iter().take(max_results).enumerate() {
            out.push_str(&format!("{}. {}\n", idx + 1, title.trim()));
            if !url.trim().is_empty() {
                out.push_str(&format!("   URL: {}\n", url.trim()));
            }
            if !snippet.trim().is_empty() {
                out.push_str(&format!("   Snippet: {}\n", snippet.trim()));
            }
            out.push('\n');
        }

        Ok(out.trim_end().to_string())
    }
}

fn collect_topics(topics: &[DdgTopic], out: &mut Vec<(String, String, String)>) {
    for topic in topics {
        if let (Some(text), Some(url)) = (&topic.text, &topic.first_url) {
            out.push((extract_title(text), url.clone(), text.clone()));
        }
        if let Some(children) = &topic.topics {
            collect_topics(children, out);
        }
    }
}

fn extract_title(text: &str) -> String {
    text.split(" - ").next().unwrap_or(text).trim().to_string()
}
