use super::Provider;
use super::types::{
    ChatRequest, ChatResponse, Choice, Message, PromptTokensDetails, ToolCall, Usage,
};
use super::{HttpClient, HttpMethod, HttpRequest, ProviderModel};
use crate::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiAuthMethod {
    ApiKey,
    ChatgptSubscription,
}

/// OpenAI provider supporting both API key auth and ChatGPT subscription tokens.
pub struct OpenAiProvider {
    auth_method: OpenAiAuthMethod,
    token: String,
    account_id: Option<String>,
    base_url: String,
}

impl OpenAiProvider {
    pub fn new_api_key(api_key: impl Into<String>) -> Self {
        Self {
            auth_method: OpenAiAuthMethod::ApiKey,
            token: api_key.into(),
            account_id: None,
            base_url: "https://api.openai.com/v1".to_string(),
        }
    }

    pub fn new_chatgpt_subscription(
        access_token: impl Into<String>,
        account_id: impl Into<String>,
    ) -> Self {
        Self {
            auth_method: OpenAiAuthMethod::ChatgptSubscription,
            token: access_token.into(),
            account_id: Some(account_id.into()),
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn auth_method(&self) -> OpenAiAuthMethod {
        self.auth_method
    }

    fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.base_url.trim_end_matches('/'))
    }

    fn to_responses_request(&self, chat_request: &ChatRequest) -> ResponsesRequest {
        let mut instructions = Vec::new();
        let mut input = Vec::new();

        for message in &chat_request.messages {
            match message.role {
                super::types::Role::System => {
                    if let Some(content) = message.content.as_deref() {
                        let trimmed = content.trim();
                        if !trimmed.is_empty() {
                            instructions.push(trimmed.to_string());
                        }
                    }
                }
                super::types::Role::User => {
                    if let Some(item) = message_to_response_item(message, "input_text") {
                        input.push(item);
                    }
                }
                super::types::Role::Assistant => {
                    if let Some(item) = message_to_response_item(message, "output_text") {
                        input.push(item);
                    }
                    if let Some(tool_calls) = &message.tool_calls {
                        for call in tool_calls {
                            input.push(ResponsesInputItem::FunctionCall {
                                call_id: call.id.clone(),
                                name: call.function.name.clone(),
                                arguments: call.function.arguments.clone(),
                            });
                        }
                    }
                }
                super::types::Role::Tool => {
                    if let (Some(call_id), Some(content)) =
                        (message.tool_call_id.as_deref(), message.content.as_deref())
                    {
                        input.push(ResponsesInputItem::FunctionCallOutput {
                            call_id: call_id.to_string(),
                            output: content.to_string(),
                        });
                    }
                }
            }
        }

        let tools = chat_request
            .tools
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|tool| ResponsesTool {
                tool_type: tool.tool_type,
                name: tool.function.name,
                description: tool.function.description,
                strict: false,
                parameters: tool.function.parameters,
            })
            .collect::<Vec<_>>();

        ResponsesRequest {
            model: chat_request.model.clone(),
            instructions: (!instructions.is_empty()).then(|| instructions.join("\n\n")),
            input,
            tools,
            tool_choice: chat_request.tools.as_ref().map(|_| "auto".to_string()),
            temperature: chat_request.temperature,
            max_output_tokens: chat_request.max_tokens,
            store: self
                .auth_method
                .eq(&OpenAiAuthMethod::ChatgptSubscription)
                .then_some(false),
            stream: self.auth_method == OpenAiAuthMethod::ChatgptSubscription,
        }
    }
}

fn message_to_response_item(message: &Message, content_type: &str) -> Option<ResponsesInputItem> {
    let content = message.content.as_deref()?.trim();
    if content.is_empty() {
        return None;
    }

    Some(ResponsesInputItem::Message {
        role: role_name(&message.role).to_string(),
        content: vec![ResponsesContentItem {
            content_type: content_type.to_string(),
            text: content.to_string(),
        }],
    })
}

fn role_name(role: &super::types::Role) -> &'static str {
    match role {
        super::types::Role::System => "system",
        super::types::Role::User => "user",
        super::types::Role::Assistant => "assistant",
        super::types::Role::Tool => "tool",
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn api_key(&self) -> &str {
        &self.token
    }

    fn auth_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![(
            "Authorization".to_string(),
            format!("Bearer {}", self.token),
        )];
        if let Some(account_id) = &self.account_id {
            headers.push(("ChatGPT-Account-Id".to_string(), account_id.clone()));
        }
        headers
    }

    fn build_http_request(&self, chat_request: &ChatRequest) -> Result<HttpRequest> {
        let body = serde_json::to_string(&self.to_responses_request(chat_request))?;
        let mut headers = self.auth_headers();
        headers.push(("Content-Type".to_string(), "application/json".to_string()));

        Ok(HttpRequest {
            url: self.responses_url(),
            method: HttpMethod::Post,
            headers,
            body: Some(body),
        })
    }

    fn parse_response(&self, body: &str) -> Result<ChatResponse> {
        let response_body = normalize_openai_response_body(body)?;
        let response: ResponsesResponse = serde_json::from_str(&response_body).map_err(|e| {
            crate::Error::provider(self.name(), format!("Invalid Responses API payload: {e}"))
        })?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut has_tool_calls = false;

        for item in response.output {
            match item {
                ResponsesOutputItem::Message {
                    role,
                    content: items,
                } if role == "assistant" => {
                    for item in items {
                        if let Some(text) = item.text() {
                            content.push_str(text);
                        }
                    }
                }
                ResponsesOutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    has_tool_calls = true;
                    tool_calls.push(ToolCall {
                        id: call_id,
                        call_type: "function".to_string(),
                        function: super::types::FunctionCall { name, arguments },
                    });
                }
                _ => {}
            }
        }

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tool_calls((!content.is_empty()).then_some(content), tool_calls)
        };

        Ok(ChatResponse {
            id: response.id,
            choices: vec![Choice {
                index: 0,
                message,
                finish_reason: Some(if has_tool_calls { "tool_calls" } else { "stop" }.to_string()),
            }],
            usage: response.usage.map(|usage| Usage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: usage.total_tokens,
                prompt_tokens_details: Some(PromptTokensDetails {
                    cached_tokens: usage
                        .input_tokens_details
                        .as_ref()
                        .and_then(|d| d.cached_tokens)
                        .or_else(|| {
                            usage
                                .input_tokens_details
                                .as_ref()
                                .and_then(|d| d.cached_input_tokens)
                        }),
                    cache_read_input_tokens: usage
                        .input_tokens_details
                        .and_then(|d| d.cached_input_tokens),
                }),
                cost: None,
            }),
            model: response.model,
        })
    }

    async fn list_models(&self, http_client: &dyn HttpClient) -> Result<Vec<ProviderModel>> {
        let request = HttpRequest {
            url: self.models_url(),
            method: HttpMethod::Get,
            headers: self.auth_headers(),
            body: None,
        };
        let response = http_client.send(request).await?;
        if response.status >= 400 {
            return Err(crate::Error::api(
                response.status,
                format!(
                    "OpenAI model list request failed: {}",
                    response.body.chars().take(500).collect::<String>()
                ),
            ));
        }

        let parsed: ModelsResponse = serde_json::from_str(&response.body).map_err(|e| {
            crate::Error::provider(self.name(), format!("Invalid model list response: {e}"))
        })?;

        let mut models = if !parsed.data.is_empty() {
            parsed
                .data
                .into_iter()
                .map(|m| ProviderModel {
                    id: m.id,
                    name: m.name,
                    description: m.description,
                })
                .collect::<Vec<_>>()
        } else {
            parsed
                .models
                .into_iter()
                .map(|m| ProviderModel {
                    id: m.slug,
                    name: m.display_name,
                    description: m.description,
                })
                .collect::<Vec<_>>()
        };

        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<ResponsesInputItem>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    tools: Vec<ResponsesTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "max_output_tokens")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<bool>,
    stream: bool,
}

fn normalize_openai_response_body(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if !trimmed.starts_with("event:") && !trimmed.starts_with("data:") {
        return Ok(body.to_string());
    }

    parse_sse_response_body(trimmed)
}

fn parse_sse_response_body(body: &str) -> Result<String> {
    let mut current_data = String::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(data) = trimmed.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            if !current_data.is_empty() {
                current_data.push('\n');
            }
            current_data.push_str(data);
            continue;
        }

        if !trimmed.is_empty() {
            continue;
        }

        if current_data.is_empty() {
            continue;
        }

        if let Some(result) = maybe_extract_completed_response(&current_data)? {
            return Ok(result);
        }
        current_data.clear();
    }

    if !current_data.is_empty()
        && let Some(result) = maybe_extract_completed_response(&current_data)?
    {
        return Ok(result);
    }

    Err(crate::Error::provider(
        "openai",
        "Streaming response did not include a response.completed event",
    ))
}

fn maybe_extract_completed_response(data: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(data).map_err(|e| {
        crate::Error::provider("openai", format!("Invalid OpenAI streaming event: {e}"))
    })?;

    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("response.completed") => value
            .get("response")
            .map(|response| serde_json::to_string(response).map_err(crate::Error::Json))
            .transpose(),
        Some("error") => Err(crate::Error::provider(
            "openai",
            value.get("error").cloned().unwrap_or(value).to_string(),
        )),
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
enum ResponsesInputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        content: Vec<ResponsesContentItem>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

#[derive(Debug, Clone, Serialize)]
struct ResponsesContentItem {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct ResponsesTool {
    #[serde(rename = "type")]
    tool_type: String,
    name: String,
    description: String,
    strict: bool,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    #[serde(default)]
    usage: Option<ResponsesUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponsesOutputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        #[serde(default)]
        content: Vec<ResponsesOutputContentItem>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponsesOutputContentItem {
    #[serde(rename = "output_text")]
    OutputText { text: String },
    #[serde(rename = "input_text")]
    InputText { text: String },
    #[serde(other)]
    Other,
}

impl ResponsesOutputContentItem {
    fn text(&self) -> Option<&str> {
        match self {
            Self::OutputText { text } | Self::InputText { text } => Some(text),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    input_tokens_details: Option<ResponsesInputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesInputTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cached_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModel>,
    #[serde(default)]
    models: Vec<CodexModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexModel {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::types::{ChatRequest, Message, ToolDefinition};

    #[test]
    fn api_key_requests_use_responses_api() {
        let provider = OpenAiProvider::new_api_key("sk-test");
        let request =
            ChatRequest::new("gpt-5.2", vec![Message::system("sys"), Message::user("hi")])
                .with_tools(vec![ToolDefinition::function(
                    "list_dir",
                    "List a directory",
                    serde_json::json!({"type":"object","properties":{}}),
                )]);

        let http = provider.build_http_request(&request).expect("request");
        assert_eq!(http.url, "https://api.openai.com/v1/responses");
        assert!(
            http.headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test")
        );
        assert!(
            !http
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("ChatGPT-Account-Id"))
        );

        let body: serde_json::Value =
            serde_json::from_str(http.body.as_deref().expect("body")).expect("json");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["stream"], false);
        assert!(body.get("store").is_none());
    }

    #[test]
    fn subscription_requests_include_account_header() {
        let provider = OpenAiProvider::new_chatgpt_subscription("access-token", "acct_123");
        let request = ChatRequest::new("gpt-5.2", vec![Message::user("hi")]);
        let http = provider.build_http_request(&request).expect("request");

        assert_eq!(http.url, "https://chatgpt.com/backend-api/codex/responses");
        assert!(
            http.headers
                .iter()
                .any(|(k, v)| k == "ChatGPT-Account-Id" && v == "acct_123")
        );
        let body: serde_json::Value =
            serde_json::from_str(http.body.as_deref().expect("body")).expect("json");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn streaming_completed_event_parses_like_standard_response() {
        let provider = OpenAiProvider::new_chatgpt_subscription("access-token", "acct_123");
        let parsed = provider
            .parse_response(
                "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\"}}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"model\":\"gpt-5.4\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n\
data: [DONE]\n\n",
            )
            .expect("parsed");

        assert_eq!(parsed.id.as_deref(), Some("resp_123"));
        assert_eq!(parsed.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(parsed.content(), Some("Hello"));
    }

    #[test]
    fn responses_payload_parses_text_and_tool_calls() {
        let provider = OpenAiProvider::new_api_key("sk-test");
        let parsed = provider
            .parse_response(
                &serde_json::json!({
                    "id": "resp_123",
                    "model": "gpt-5.2",
                    "output": [
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": "Hello"}]
                        },
                        {
                            "type": "function_call",
                            "call_id": "call_1",
                            "name": "list_dir",
                            "arguments": "{\"path\":\".\"}"
                        }
                    ],
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 4,
                        "total_tokens": 14,
                        "input_tokens_details": {
                            "cached_input_tokens": 2
                        }
                    }
                })
                .to_string(),
            )
            .expect("parsed");

        assert_eq!(parsed.id.as_deref(), Some("resp_123"));
        assert_eq!(parsed.model.as_deref(), Some("gpt-5.2"));
        assert_eq!(parsed.content(), Some("Hello"));
        assert!(parsed.has_tool_calls());
        assert_eq!(parsed.tool_calls().expect("tool calls")[0].id, "call_1");
        assert_eq!(parsed.usage.expect("usage").cached_tokens(), 2);
    }

    #[test]
    fn invalid_responses_payload_returns_provider_error() {
        let provider = OpenAiProvider::new_api_key("sk-test");
        let err = provider.parse_response("{not-json").expect_err("invalid");
        assert!(err.to_string().contains("Invalid Responses API payload"));
    }
}
