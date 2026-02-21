pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod list_dir;
pub mod shell_exec;
pub mod webfetch;
pub mod websearch;

use zagent_core::tools::ToolRegistry;

/// Register all native tools into a ToolRegistry
pub fn register_all_tools(working_dir: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    registry.register(Box::new(shell_exec::ShellExecTool::new(working_dir)));
    registry.register(Box::new(file_edit::FileEditTool::new()));
    registry.register(Box::new(file_read::FileReadTool::new()));
    registry.register(Box::new(file_write::FileWriteTool::new()));
    registry.register(Box::new(list_dir::ListDirTool::new()));
    registry.register(Box::new(websearch::WebSearchTool::new()));
    registry.register(Box::new(webfetch::WebFetchTool::new()));

    registry
}
