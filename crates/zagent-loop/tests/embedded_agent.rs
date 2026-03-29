use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use zagent_core::provider::types::{
    ChatRequest, ChatResponse, Choice, FunctionCall, Message, ToolCall,
};
use zagent_core::provider::{HttpClient, HttpRequest, HttpResponse, Provider};
use zagent_loop::{LoopAgent, LoopAgentOptions, MemoryFileSystem};

struct TestProvider;

#[async_trait]
impl Provider for TestProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn base_url(&self) -> &str {
        "https://example.invalid"
    }

    fn api_key(&self) -> &str {
        "test-key"
    }
}

#[derive(Default)]
struct ScriptedHttpClient {
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl HttpClient for ScriptedHttpClient {
    async fn send(&self, request: HttpRequest) -> zagent_core::Result<HttpResponse> {
        let chat_request: ChatRequest =
            serde_json::from_str(request.body.as_deref().unwrap_or("{}"))?;
        self.requests
            .lock()
            .expect("requests lock")
            .push(chat_request.clone());

        let tool_results = chat_request
            .messages
            .iter()
            .filter(|msg| msg.role == zagent_core::provider::types::Role::Tool)
            .count();

        let response = if tool_results == 0 {
            ChatResponse {
                id: Some("resp-tools".to_string()),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant_with_tool_calls(
                        None,
                        vec![ToolCall {
                            id: "call_read".to_string(),
                            call_type: "function".to_string(),
                            function: FunctionCall {
                                name: "file_read".to_string(),
                                arguments: serde_json::json!({ "path": "project/readme.md" })
                                    .to_string(),
                            },
                        }],
                    ),
                    finish_reason: Some("tool_calls".to_string()),
                }],
                usage: None,
                model: Some(chat_request.model),
            }
        } else {
            ChatResponse {
                id: Some("resp-final".to_string()),
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant("read complete"),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
                model: Some(chat_request.model),
            }
        };

        Ok(HttpResponse {
            status: 200,
            body: serde_json::to_string(&response)?,
            headers: Vec::new(),
        })
    }
}

#[tokio::test]
async fn embedded_agent_reads_files_from_injected_filesystem() {
    let workspace_fs = Arc::new(MemoryFileSystem::from_files([
        (
            "AGENTS.md",
            "VFS rule: stay inside the injected workspace.\n",
        ),
        ("project/readme.md", "hello from embedded runtime\n"),
    ]));

    let http_client = Arc::new(ScriptedHttpClient::default());
    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(TestProvider));

    let mut options = LoopAgentOptions::new("openai", "test-model", "embedded", ".");
    options.max_turns = 4;

    let agent = LoopAgent::new(
        http_client.clone(),
        providers,
        workspace_fs,
        options,
    )
    .expect("agent should build");

    let response = agent
        .send_input("Summarize the readme")
        .await
        .expect("agent should run");

    assert_eq!(response.output, "read complete");
    assert_eq!(response.tool_calls, 1);
    assert!(agent.tool_names().contains(&"file_read"));

    let requests = http_client.requests.lock().expect("requests lock");
    let system_prompt = requests
        .first()
        .and_then(|request| request.messages.first())
        .and_then(|message| message.content.as_deref())
        .unwrap_or("");
    assert!(system_prompt.contains("VFS rule: stay inside the injected workspace."));
}
