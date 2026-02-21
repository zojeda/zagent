use serde::{Deserialize, Serialize};

// ============================================================================
// Request types
// ============================================================================

/// Top-level chat completion request (OpenAI-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        if !tools.is_empty() {
            self.tools = Some(tools);
            self.tool_choice = Some(serde_json::json!("auto"));
        }
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }
}

// ============================================================================
// Message types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Optional name field (used by some providers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Optional provider reasoning payload (e.g., reasoning_details)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_details: Option<serde_json::Value>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_details: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_details: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_details: None,
        }
    }

    pub fn assistant_with_tool_calls(content: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
            name: None,
            reasoning_details: None,
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            reasoning_details: None,
        }
    }

    pub fn with_reasoning_details(mut self, reasoning_details: Option<serde_json::Value>) -> Self {
        self.reasoning_details = reasoning_details;
        self
    }
}

// ============================================================================
// Tool types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,

    #[serde(rename = "type")]
    pub call_type: String,

    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON string — the model returns this as a string
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            tool_type: "function".to_string(),
            function: FunctionSchema {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

// ============================================================================
// Response types
// ============================================================================

/// Top-level chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: Option<String>,
    pub choices: Vec<Choice>,

    #[serde(default)]
    pub usage: Option<Usage>,

    /// Model that was actually used (may differ from requested model)
    pub model: Option<String>,
}

impl ChatResponse {
    /// Get the first choice's message content (if any)
    pub fn content(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|c| c.message.content.as_deref())
    }

    /// Get tool calls from the first choice (if any)
    pub fn tool_calls(&self) -> Option<&Vec<ToolCall>> {
        self.choices
            .first()
            .and_then(|c| c.message.tool_calls.as_ref())
    }

    /// Check if the response contains tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls().map(|tc| !tc.is_empty()).unwrap_or(false)
    }

    /// Get the finish reason
    pub fn finish_reason(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, deserialize_with = "de_opt_f64_from_any")]
    pub cost: Option<f64>,
}

impl Usage {
    pub fn cached_tokens(&self) -> u64 {
        self.prompt_tokens_details
            .as_ref()
            .map(PromptTokensDetails::cached_tokens)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
}

impl PromptTokensDetails {
    pub fn cached_tokens(&self) -> u64 {
        self.cached_tokens
            .unwrap_or_else(|| self.cache_read_input_tokens.unwrap_or(0))
    }
}

fn de_opt_f64_from_any<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => n.as_f64(),
        Some(serde_json::Value::String(s)) => s.parse::<f64>().ok(),
        _ => None,
    })
}

// ============================================================================
// Error response (from API)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub code: Option<String>,
}
