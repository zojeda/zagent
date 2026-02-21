use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// Apply a unified diff to an existing file.
pub struct FileEditTool;

impl FileEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to an existing file. Use this for surgical edits instead of rewriting the whole file."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to edit"
                },
                "diff": {
                    "type": "string",
                    "description": "Unified diff hunk(s) for this single file (lines starting with @@, +, -, and space)"
                }
            },
            "required": ["path", "diff"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_edit", "Missing 'path' parameter"))?;

        let diff = args
            .get("diff")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_edit", "Missing 'diff' parameter"))?;

        let original = fs::read_to_string(path).await.map_err(|e| {
            zagent_core::Error::tool("file_edit", format!("Failed to read '{path}': {e}"))
        })?;

        let updated = apply_unified_diff(&original, diff)?;

        fs::write(path, &updated).await.map_err(|e| {
            zagent_core::Error::tool("file_edit", format!("Failed to write '{path}': {e}"))
        })?;

        Ok(format!(
            "Successfully applied diff to {path} ({} -> {} bytes)",
            original.len(),
            updated.len()
        ))
    }
}

fn apply_unified_diff(original: &str, diff: &str) -> Result<String> {
    let input_lines: Vec<&str> = original.lines().collect();
    let mut src_idx: usize = 0;
    let mut out: Vec<String> = Vec::new();

    let diff_lines: Vec<&str> = diff.lines().collect();
    let mut i = 0usize;
    let mut saw_hunk = false;

    while i < diff_lines.len() {
        let line = diff_lines[i];
        if !line.starts_with("@@") {
            i += 1;
            continue;
        }
        saw_hunk = true;
        let (old_start, _old_count) = parse_hunk_header(line)?;
        let target_idx = old_start.saturating_sub(1);
        if target_idx < src_idx || target_idx > input_lines.len() {
            return Err(zagent_core::Error::tool(
                "file_edit",
                format!("Invalid hunk target line {old_start}"),
            ));
        }

        while src_idx < target_idx {
            out.push(input_lines[src_idx].to_string());
            src_idx += 1;
        }

        i += 1;
        while i < diff_lines.len() && !diff_lines[i].starts_with("@@") {
            let hunk_line = diff_lines[i];
            if hunk_line.starts_with("\\ No newline at end of file") {
                i += 1;
                continue;
            }
            let (prefix, body) =
                hunk_line.split_at(hunk_line.chars().next().map(|c| c.len_utf8()).unwrap_or(0));
            match prefix {
                " " => {
                    let current = input_lines.get(src_idx).copied().unwrap_or_default();
                    if current != body {
                        return Err(zagent_core::Error::tool(
                            "file_edit",
                            format!(
                                "Context mismatch at source line {}: expected '{}', got '{}'",
                                src_idx + 1,
                                body,
                                current
                            ),
                        ));
                    }
                    out.push(body.to_string());
                    src_idx += 1;
                }
                "-" => {
                    let current = input_lines.get(src_idx).copied().unwrap_or_default();
                    if current != body {
                        return Err(zagent_core::Error::tool(
                            "file_edit",
                            format!(
                                "Delete mismatch at source line {}: expected '{}', got '{}'",
                                src_idx + 1,
                                body,
                                current
                            ),
                        ));
                    }
                    src_idx += 1;
                }
                "+" => {
                    out.push(body.to_string());
                }
                _ => {
                    return Err(zagent_core::Error::tool(
                        "file_edit",
                        format!("Unexpected diff line: {hunk_line}"),
                    ));
                }
            }
            i += 1;
        }
    }

    if !saw_hunk {
        return Err(zagent_core::Error::tool(
            "file_edit",
            "Diff did not contain any unified hunk (@@ ... @@)",
        ));
    }

    while src_idx < input_lines.len() {
        out.push(input_lines[src_idx].to_string());
        src_idx += 1;
    }

    Ok(out.join("\n"))
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize)> {
    let Some(rest) = line.strip_prefix("@@") else {
        return Err(zagent_core::Error::tool(
            "file_edit",
            format!("Invalid hunk header: {line}"),
        ));
    };
    let Some(mid) = rest.find("@@") else {
        return Err(zagent_core::Error::tool(
            "file_edit",
            format!("Invalid hunk header: {line}"),
        ));
    };
    let spec = rest[..mid].trim();
    let mut parts = spec.split_whitespace();
    let old = parts.next().unwrap_or_default();
    let _new = parts.next().unwrap_or_default();
    parse_range(old)
}

fn parse_range(token: &str) -> Result<(usize, usize)> {
    let s = token
        .strip_prefix('-')
        .or_else(|| token.strip_prefix('+'))
        .unwrap_or(token);
    let mut it = s.split(',');
    let start =
        it.next().unwrap_or("0").parse::<usize>().map_err(|e| {
            zagent_core::Error::tool("file_edit", format!("Invalid hunk start: {e}"))
        })?;
    let count = it
        .next()
        .map(|c| c.parse::<usize>())
        .transpose()
        .map_err(|e| zagent_core::Error::tool("file_edit", format!("Invalid hunk count: {e}")))?
        .unwrap_or(1);
    Ok((start, count))
}
