use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// Read file contents, optionally with line range.
pub struct FileReadTool;

impl FileReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally specify start_line and end_line \
         for reading a portion of the file (1-indexed, inclusive). \
         Returns the file contents as text."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to read"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line number (1-indexed, inclusive). If omitted, reads from the start."
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line number (1-indexed, inclusive). If omitted, reads to the end."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| zagent_core::Error::tool("file_read", "Missing 'path' parameter"))?;

        let content = fs::read_to_string(path).await.map_err(|e| {
            zagent_core::Error::tool("file_read", format!("Failed to read '{path}': {e}"))
        })?;

        let start_line = args
            .get("start_line")
            .and_then(|v| v.as_u64())
            .map(|l| l.max(1) as usize);
        let end_line = args
            .get("end_line")
            .and_then(|v| v.as_u64())
            .map(|l| l as usize);

        if start_line.is_some() || end_line.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let start = start_line.unwrap_or(1).saturating_sub(1); // convert to 0-indexed
            let end = end_line.unwrap_or(lines.len()).min(lines.len());

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
            // Return with line numbers
            let numbered: Vec<String> = content
                .lines()
                .enumerate()
                .map(|(i, line)| format!("{:>4} | {}", i + 1, line))
                .collect();
            Ok(numbered.join("\n"))
        }
    }
}
