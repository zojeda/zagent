use async_trait::async_trait;
use serde_json::Value;
use tokio::fs;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// Write content to a file, creating it and its parent directories if needed.
pub struct FileWriteTool;

impl FileWriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file and any parent directories if they don't exist. \
         Overwrites existing content. Use this for creating new files or replacing file contents entirely."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
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

        // Create parent directories if needed
        if let Some(parent) = std::path::Path::new(path).parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).await.map_err(|e| {
                zagent_core::Error::tool(
                    "file_write",
                    format!("Failed to create directory '{}': {e}", parent.display()),
                )
            })?;
        }

        fs::write(path, content).await.map_err(|e| {
            zagent_core::Error::tool("file_write", format!("Failed to write '{path}': {e}"))
        })?;

        let line_count = content.lines().count();
        let byte_count = content.len();
        Ok(format!(
            "Successfully wrote {byte_count} bytes ({line_count} lines) to {path}"
        ))
    }
}
