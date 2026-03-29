use async_trait::async_trait;
use serde_json::Value;

use zagent_core::Result;
use zagent_core::tools::Tool;

use crate::fs::SharedFileSystem;

/// List directory contents.
pub struct ListDirTool {
    file_system: SharedFileSystem,
}

impl ListDirTool {
    pub fn new(file_system: SharedFileSystem) -> Self {
        Self { file_system }
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

        let mut entries = self
            .file_system
            .list_dir(path, recursive, max_depth)
            .await
            .map_err(|e| zagent_core::Error::tool("list_dir", e.to_string()))?;
        entries.retain(|entry| {
            !(entry.depth == 0
                && entry.name.starts_with('.')
                && entry.name != ".env"
                && entry.name != ".gitignore")
        });

        if entries.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            Ok(entries
                .into_iter()
                .map(|entry| {
                    let indent = "  ".repeat(entry.depth);
                    if entry.is_dir {
                        format!("{indent}{}/", entry.name)
                    } else {
                        let size_str = format_size(entry.size);
                        format!("{indent}{}  ({size_str})", entry.name)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
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
