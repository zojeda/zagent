use async_trait::async_trait;
use serde_json::Value;

use crate::Result;
use crate::provider::types::ToolDefinition;

/// Trait that all agent tools must implement.
///
/// Each tool provides its schema (for the LLM) and an execution method.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique identifier for this tool
    fn name(&self) -> &str;

    /// Human-readable description (sent to the LLM)
    fn description(&self) -> &str;

    /// JSON Schema for the tool parameters (OpenAI function calling format)
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given arguments
    async fn execute(&self, args: Value) -> Result<String>;

    /// Build the OpenAI-compatible tool definition
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition::function(self.name(), self.description(), self.parameters_schema())
    }
}

/// Registry that holds all available tools and dispatches execution.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a new tool
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Get all tool definitions for sending to the LLM
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.to_definition()).collect()
    }

    /// Execute a tool by name with the given arguments
    pub async fn execute(&self, name: &str, args: Value) -> Result<String> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| crate::Error::tool(name, format!("Unknown tool: {name}")))?;

        tool.execute(args).await
    }

    /// Get the number of registered tools
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// List all tool names
    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }
}
