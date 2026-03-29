pub mod mcp;
pub mod shell_exec;
pub mod webfetch;
pub mod websearch;

use std::sync::Arc;

use zagent_core::tools::ToolRegistry;
use zagent_loop::register_file_tools;

use crate::fs::{RootedHostFileSystem, SharedFileSystem};

pub async fn register_all_tools(
    working_dir: &str,
    mcp_manager: Option<std::sync::Arc<crate::mcp::McpManager>>,
) -> ToolRegistry {
    let file_system: SharedFileSystem = Arc::new(
        RootedHostFileSystem::new(working_dir).expect("working directory should be a valid root"),
    );
    register_all_tools_with_filesystem(file_system, working_dir, mcp_manager).await
}

pub async fn register_all_tools_with_filesystem(
    file_system: SharedFileSystem,
    working_dir: &str,
    mcp_manager: Option<std::sync::Arc<crate::mcp::McpManager>>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(shell_exec::ShellExecTool::new(working_dir)));
    register_file_tools(&mut registry, file_system);
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
pub fn register_wasi_tools(working_dir: &str) -> ToolRegistry {
    let file_system: SharedFileSystem = Arc::new(
        RootedHostFileSystem::new(working_dir).expect("working directory should be a valid root"),
    );
    register_wasi_tools_with_filesystem(file_system)
}

pub fn register_wasi_tools_with_filesystem(file_system: SharedFileSystem) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    register_file_tools(&mut registry, file_system);
    registry.register(Box::new(websearch::WebSearchTool::new()));
    registry.register(Box::new(webfetch::WebFetchTool::new()));

    registry
}
