pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod list_dir;
pub mod mcp;
pub mod shell_exec;
pub mod webfetch;
pub mod websearch;

use zagent_core::tools::ToolRegistry;

pub async fn register_all_tools(
    working_dir: &str,
    mcp_manager: Option<std::sync::Arc<crate::mcp::McpManager>>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(shell_exec::ShellExecTool::new(working_dir)));
    registry.register(Box::new(file_edit::FileEditTool::new()));
    registry.register(Box::new(file_read::FileReadTool::new()));
    registry.register(Box::new(file_write::FileWriteTool::new()));
    registry.register(Box::new(list_dir::ListDirTool::new()));
    registry.register(Box::new(websearch::WebSearchTool::new()));
    registry.register(Box::new(webfetch::WebFetchTool::new()));
    if let Some(manager) = mcp_manager {
        for tool_name in manager.connected_tool_names().await {
            registry.register(Box::new(mcp::McpTool::new(tool_name, manager.clone())));
        }
    }

    registry
}

/// Tool registry for WASI mode.
///
/// This intentionally excludes host shell execution to keep the runtime constrained.
pub fn register_wasi_tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(file_edit::FileEditTool::new()));
    registry.register(Box::new(file_read::FileReadTool::new()));
    registry.register(Box::new(file_write::FileWriteTool::new()));
    registry.register(Box::new(list_dir::ListDirTool::new()));
    registry.register(Box::new(websearch::WebSearchTool::new()));
    registry.register(Box::new(webfetch::WebFetchTool::new()));

    registry
}
