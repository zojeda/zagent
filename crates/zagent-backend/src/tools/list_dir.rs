use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use tokio::fs;

use zagent_core::Result;
use zagent_core::tools::Tool;

/// List directory contents.
pub struct ListDirTool;

impl Default for ListDirTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ListDirTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List the contents of a directory. Shows file names with type indicators \
         (trailing / for directories). Optionally list recursively."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the directory to list"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, list recursively (default: false)"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum recursion depth (default: 3, only used if recursive is true)"
                }
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

        let path = Path::new(path);
        if !path.exists() {
            return Err(zagent_core::Error::tool(
                "list_dir",
                format!("Path '{}' does not exist", path.display()),
            ));
        }
        if !path.is_dir() {
            return Err(zagent_core::Error::tool(
                "list_dir",
                format!("'{}' is not a directory", path.display()),
            ));
        }

        let mut entries = Vec::new();
        list_dir_inner(path, &mut entries, recursive, 0, max_depth).await?;
        entries.sort();

        if entries.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            Ok(entries.join("\n"))
        }
    }
}

fn list_dir_inner<'a>(
    dir: &'a Path,
    entries: &'a mut Vec<String>,
    recursive: bool,
    depth: usize,
    max_depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut read_dir = fs::read_dir(dir).await.map_err(|e| {
            zagent_core::Error::tool(
                "list_dir",
                format!("Failed to read '{}': {e}", dir.display()),
            )
        })?;

        let indent = "  ".repeat(depth);

        while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
            zagent_core::Error::tool("list_dir", format!("Failed to read entry: {e}"))
        })? {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            // Skip hidden files at depth 0 unless they're important
            if depth == 0 && name.starts_with('.') && name != ".env" && name != ".gitignore" {
                continue;
            }

            let metadata = entry.metadata().await.map_err(|e| {
                zagent_core::Error::tool("list_dir", format!("Failed to read metadata: {e}"))
            })?;

            if metadata.is_dir() {
                entries.push(format!("{indent}{name}/"));
                if recursive && depth < max_depth {
                    list_dir_inner(&entry.path(), entries, recursive, depth + 1, max_depth).await?;
                }
            } else {
                let size = metadata.len();
                let size_str = format_size(size);
                entries.push(format!("{indent}{name}  ({size_str})"));
            }
        }

        Ok(())
    })
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
