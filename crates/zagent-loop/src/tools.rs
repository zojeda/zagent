use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zagent_core::Result;
use zagent_core::fs::AgentFileSystem;
use zagent_core::tools::{Tool, ToolRegistry};

pub fn build_file_tools(file_system: Arc<dyn AgentFileSystem>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    register_file_tools(&mut registry, file_system);
    registry
}

pub fn register_file_tools(registry: &mut ToolRegistry, file_system: Arc<dyn AgentFileSystem>) {
    registry.register(Box::new(FileEditTool::new(file_system.clone())));
    registry.register(Box::new(FileReadTool::new(file_system.clone())));
    registry.register(Box::new(FileWriteTool::new(file_system.clone())));
    registry.register(Box::new(ListDirTool::new(file_system)));
}

pub struct FileReadTool {
    file_system: Arc<dyn AgentFileSystem>,
}

impl FileReadTool {
    pub fn new(file_system: Arc<dyn AgentFileSystem>) -> Self {
        Self { file_system }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally specify start_line and end_line for reading a portion of the file (1-indexed, inclusive). Returns the file contents as text."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "start_line": { "type": "integer" },
                "end_line": { "type": "integer" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_read", "Missing 'path' parameter"))?;

        let content = self.file_system.read_to_string(path).await.map_err(|e| {
            zagent_core::Error::tool("file_read", format!("Failed to read '{path}': {e}"))
        })?;

        let start_line = args
            .get("start_line")
            .and_then(|v| v.as_u64())
            .map(|l| l.max(1) as usize);
        let end_line = args
            .get("end_line")
            .and_then(|v| v.as_u64())
            .map(|l| l.max(1) as usize);

        if start_line.is_some() || end_line.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let start_1 = start_line.unwrap_or(1);
            let end_1 = end_line.unwrap_or(lines.len().max(start_1));
            if end_line.is_some() && end_1 < start_1 {
                return Err(zagent_core::Error::tool(
                    "file_read",
                    format!(
                        "Invalid line range: start_line ({start_1}) must be <= end_line ({end_1})"
                    ),
                ));
            }
            let start = start_1.saturating_sub(1);
            let end = end_1.min(lines.len());

            if start >= lines.len() {
                return Ok(format!(
                    "(file has {} lines, requested start line {})",
                    lines.len(),
                    start + 1
                ));
            }

            let selected: Vec<String> = lines[start..end]
                .iter()
                .enumerate()
                .map(|(i, line)| format!("{:>4} | {}", start + i + 1, line))
                .collect();
            Ok(selected.join("\n"))
        } else {
            Ok(content
                .lines()
                .enumerate()
                .map(|(i, line)| format!("{:>4} | {}", i + 1, line))
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }
}

pub struct FileWriteTool {
    file_system: Arc<dyn AgentFileSystem>,
}

impl FileWriteTool {
    pub fn new(file_system: Arc<dyn AgentFileSystem>) -> Self {
        Self { file_system }
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file and any parent directories if they don't exist. Overwrites existing content."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_write", "Missing 'path' parameter"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_write", "Missing 'content' parameter"))?;

        self.file_system
            .write_string(path, content)
            .await
            .map_err(|e| {
                zagent_core::Error::tool("file_write", format!("Failed to write '{path}': {e}"))
            })?;

        Ok(format!(
            "Successfully wrote {} bytes ({}) lines to {path}",
            content.len(),
            content.lines().count()
        ))
    }
}

pub struct FileEditTool {
    file_system: Arc<dyn AgentFileSystem>,
}

impl FileEditTool {
    pub fn new(file_system: Arc<dyn AgentFileSystem>) -> Self {
        Self { file_system }
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to an existing file."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "diff": { "type": "string" }
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

        let original = self.file_system.read_to_string(path).await.map_err(|e| {
            zagent_core::Error::tool("file_edit", format!("Failed to read '{path}': {e}"))
        })?;
        let updated = apply_unified_diff(&original, diff)?;
        self.file_system
            .write_string(path, &updated)
            .await
            .map_err(|e| {
                zagent_core::Error::tool("file_edit", format!("Failed to write '{path}': {e}"))
            })?;
        Ok(format!(
            "Successfully applied diff to {path} ({} -> {} bytes)",
            original.len(),
            updated.len()
        ))
    }
}

pub struct ListDirTool {
    file_system: Arc<dyn AgentFileSystem>,
}

impl ListDirTool {
    pub fn new(file_system: Arc<dyn AgentFileSystem>) -> Self {
        Self { file_system }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory. Optionally list recursively."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "recursive": { "type": "boolean" },
                "max_depth": { "type": "integer" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("list_dir", "Missing 'path' parameter"))?;
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;

        let entries = self
            .file_system
            .list_dir(path, recursive, max_depth)
            .await
            .map_err(|e| zagent_core::Error::tool("list_dir", e.to_string()))?;

        if entries.is_empty() {
            return Ok("(empty directory)".to_string());
        }

        Ok(entries
            .into_iter()
            .map(|entry| {
                let indent = "  ".repeat(entry.depth);
                if entry.is_dir {
                    format!("{indent}{}/", entry.name)
                } else {
                    format!("{indent}{}  ({})", entry.name, format_size(entry.size))
                }
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
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
        let (old_start, _) = parse_hunk_header(line)?;
        let target_idx = old_start.saturating_sub(1);
        if target_idx > input_lines.len() {
            return Err(zagent_core::Error::tool(
                "file_edit",
                format!(
                    "Hunk starts at line {}, but file only has {} lines",
                    old_start,
                    input_lines.len()
                ),
            ));
        }

        while src_idx < target_idx {
            out.push(input_lines[src_idx].to_string());
            src_idx += 1;
        }

        i += 1;
        while i < diff_lines.len() {
            let hline = diff_lines[i];
            if hline.starts_with("@@") {
                break;
            }
            if hline.starts_with("---") || hline.starts_with("+++") {
                i += 1;
                continue;
            }
            let (kind, text) = hline.split_at(1);
            match kind {
                " " => {
                    let expected = input_lines.get(src_idx).copied().ok_or_else(|| {
                        zagent_core::Error::tool(
                            "file_edit",
                            "Patch context extends beyond end of file",
                        )
                    })?;
                    if expected != text {
                        return Err(zagent_core::Error::tool(
                            "file_edit",
                            format!(
                                "Patch context mismatch at line {}: expected {:?}, got {:?}",
                                src_idx + 1,
                                expected,
                                text
                            ),
                        ));
                    }
                    out.push(expected.to_string());
                    src_idx += 1;
                }
                "-" => {
                    let expected = input_lines.get(src_idx).copied().ok_or_else(|| {
                        zagent_core::Error::tool(
                            "file_edit",
                            "Patch deletion extends beyond end of file",
                        )
                    })?;
                    if expected != text {
                        return Err(zagent_core::Error::tool(
                            "file_edit",
                            format!(
                                "Patch deletion mismatch at line {}: expected {:?}, got {:?}",
                                src_idx + 1,
                                expected,
                                text
                            ),
                        ));
                    }
                    src_idx += 1;
                }
                "+" => out.push(text.to_string()),
                "\\" => {}
                _ => {
                    return Err(zagent_core::Error::tool(
                        "file_edit",
                        format!("Unsupported diff line: {hline}"),
                    ));
                }
            }
            i += 1;
        }
    }

    if !saw_hunk {
        return Err(zagent_core::Error::tool(
            "file_edit",
            "Diff did not contain any hunks",
        ));
    }

    while src_idx < input_lines.len() {
        out.push(input_lines[src_idx].to_string());
        src_idx += 1;
    }

    Ok(out.join("\n"))
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize)> {
    let header = line
        .strip_prefix("@@ -")
        .and_then(|rest| rest.strip_suffix(" @@"))
        .ok_or_else(|| {
            zagent_core::Error::tool("file_edit", format!("Invalid hunk header: {line}"))
        })?;
    let mut parts = header.split(" +");
    let old = parts.next().ok_or_else(|| {
        zagent_core::Error::tool("file_edit", format!("Invalid hunk header: {line}"))
    })?;
    let new = parts.next().ok_or_else(|| {
        zagent_core::Error::tool("file_edit", format!("Invalid hunk header: {line}"))
    })?;
    Ok((parse_hunk_range(old)?, parse_hunk_range(new)?))
}

fn parse_hunk_range(range: &str) -> Result<usize> {
    let start = range.split(',').next().unwrap_or_default();
    start.parse::<usize>().map_err(|_| {
        zagent_core::Error::tool("file_edit", format!("Invalid hunk range start: {range}"))
    })
}
