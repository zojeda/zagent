pub fn format_tool_call_text(tool_name: &str, arguments: &str) -> String {
    if tool_name == "shell_exec"
        && let Some(line) = format_shell_exec_call(arguments)
    {
        return line;
    }
    let pretty_args = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| arguments.to_string());
    format!("🔧 {tool_name}\nargs:\n{pretty_args}")
}

pub fn format_reasoning_details(reasoning: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    collect_reasoning_text(reasoning, &mut parts);
    let text = parts
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() { String::new() } else { text }
}

pub fn infer_language(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or_default();
    match ext {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "ts" => "ts",
        "tsx" => "tsx",
        "js" => "js",
        "jsx" => "jsx",
        "py" => "python",
        "sh" => "bash",
        "md" => "markdown",
        "html" => "html",
        "css" => "css",
        _ => "text",
    }
}

fn format_shell_exec_call(arguments: &str) -> Option<String> {
    let args = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let mut parts = vec![format!("command={}", shell_quote(command))];
    if let Some(working_dir) = args
        .get("working_dir")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("working_dir={}", shell_quote(working_dir)));
    }
    if let Some(timeout) = args.get("timeout_secs").and_then(|v| v.as_u64())
        && timeout != 60
    {
        parts.push(format!("timeout_secs={timeout}"));
    }

    Some(format!("🔧 shell_exec {}", parts.join(" ")))
}

fn shell_quote(value: &str) -> String {
    let compact = value.replace('\n', "\\n");
    format!("\"{}\"", compact.replace('"', "\\\""))
}

fn collect_reasoning_text(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => {
            out.push(s.clone());
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_reasoning_text(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let kind = map.get("type").and_then(|v| v.as_str()).unwrap_or_default();
            if kind == "reasoning.encrypted" {
                return;
            }
            if kind == "reasoning.summary" {
                if let Some(summary) = map.get("summary") {
                    collect_reasoning_text(summary, out);
                } else if let Some(serde_json::Value::String(text)) = map.get("text") {
                    out.push(text.clone());
                }
                return;
            }

            if let Some(serde_json::Value::String(text)) = map.get("text") {
                out.push(text.clone());
            }
            if let Some(summary) = map.get("summary") {
                collect_reasoning_text(summary, out);
            }
            for (key, v) in map {
                if matches!(
                    key.as_str(),
                    "type" | "format" | "index" | "id" | "data" | "summary" | "text"
                ) {
                    continue;
                }
                collect_reasoning_text(v, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::format_reasoning_details;
    use serde_json::json;

    #[test]
    fn strips_encrypted_reasoning_entries() {
        let reasoning = json!([
            {
                "type": "reasoning.summary",
                "summary": "**Providing poem markdown**"
            },
            {
                "type": "reasoning.encrypted",
                "data": "gAAAAABplHztyYvpJ84X5MNAy6m747DU..."
            }
        ]);

        let text = format_reasoning_details(&reasoning);
        assert_eq!(text, "**Providing poem markdown**");
    }

    #[test]
    fn keeps_plain_text_reasoning() {
        let reasoning = json!({
            "format": "openai-responses-v1",
            "text": "Thinking about tool strategy"
        });

        let text = format_reasoning_details(&reasoning);
        assert_eq!(text, "Thinking about tool strategy");
    }
}
