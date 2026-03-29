pub mod shell_exec;
pub mod webfetch;
pub mod websearch;

use std::sync::Arc;

use zagent_core::tools::ToolRegistry;
use zagent_loop::{HostFileSystem, register_file_tools};

/// Register all native tools into a ToolRegistry
pub fn register_all_tools(working_dir: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(shell_exec::ShellExecTool::new(working_dir)));
    register_file_tools(&mut registry, Arc::new(HostFileSystem));
    registry.register(Box::new(websearch::WebSearchTool::new()));
    registry.register(Box::new(webfetch::WebFetchTool::new()));

    registry
}
