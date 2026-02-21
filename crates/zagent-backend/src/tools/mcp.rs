use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use zagent_core::tools::Tool;

use crate::mcp::McpManager;

pub struct McpTool {
    tool_name: String,
    manager: Arc<McpManager>,
}

impl McpTool {
    pub fn new(tool_name: String, manager: Arc<McpManager>) -> Self {
        Self { tool_name, manager }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "Tool exposed by a connected MCP server."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": true
        })
    }

    async fn execute(&self, args: Value) -> zagent_core::Result<String> {
        self.manager.call_tool(&self.tool_name, args).await
    }
}
