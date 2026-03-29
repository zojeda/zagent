use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;
use zagent_backend::engine::{
    BackendEngine, EmbeddedBackendOptions, RuntimeTarget, SessionStoreTarget,
};
use zagent_backend::runtime::RuntimeBundle;
use zagent_backend::tools::register_wasi_tools_with_filesystem;
use zagent_core::agent::AgentConfig;
use zagent_core::provider::types::{ChatRequest, ChatResponse, Choice, Message};
use zagent_core::provider::{HttpClient, HttpMethod, HttpRequest, HttpResponse, Provider};
use zagent_core::session::SessionStore;
use zagent_loop::{InMemorySessionStore, MemoryFileSystem};

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
struct CapturingHttpClient {
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl HttpClient for CapturingHttpClient {
    async fn send(&self, request: HttpRequest) -> zagent_core::Result<HttpResponse> {
        match request.method {
            HttpMethod::Post => {}
            HttpMethod::Get => panic!("expected a POST chat request"),
        }
        let chat_request: ChatRequest =
            serde_json::from_str(request.body.as_deref().unwrap_or("{}"))?;
        self.requests.lock().await.push(chat_request.clone());

        let response = ChatResponse {
            id: Some("resp-1".to_string()),
            choices: vec![Choice {
                index: 0,
                message: Message::assistant("embedded ok"),
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
            model: Some(chat_request.model),
        };

        Ok(HttpResponse {
            status: 200,
            body: serde_json::to_string(&response)?,
            headers: Vec::new(),
        })
    }
}

#[tokio::test]
async fn embedded_engine_uses_injected_workspace_filesystem_for_prompt_and_tools() {
    let workspace_fs = Arc::new(MemoryFileSystem::from_files([
        ("AGENTS.md", "VFS rule: only visible through injected fs.\n"),
        ("project/readme.md", "hello from virtual workspace\n"),
    ]));

    let http_client = Arc::new(CapturingHttpClient::default());
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let tools = Arc::new(register_wasi_tools_with_filesystem(workspace_fs.clone()));
    let runtime = RuntimeBundle {
        target: RuntimeTarget::Wasi,
        http_client: http_client.clone(),
        session_store: session_store.clone(),
        session_event_store: Some(session_store),
        session_admin_store: None,
        tools,
        workspace_fs: Some(workspace_fs.clone()),
        mcp_manager: None,
    };

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(TestProvider));

    let options = EmbeddedBackendOptions {
        provider_name: "openai".to_string(),
        config: AgentConfig {
            model: "test-model".to_string(),
            max_turns: 2,
            ..AgentConfig::default()
        },
        working_dir: ".".to_string(),
        session_store: SessionStoreTarget::Memory,
        session_dir: ".".to_string(),
        resume_session: None,
        new_session: Some("embedded".to_string()),
    };

    let engine = BackendEngine::new_embedded(providers, runtime, options)
        .await
        .expect("embedded engine should build");

    let response = engine
        .send_input("summarize the workspace rules")
        .await
        .expect("embedded engine should run");
    assert_eq!(response.output, "embedded ok");

    let requests = http_client.requests.lock().await;
    let system_prompt = requests
        .first()
        .and_then(|req| req.messages.first())
        .and_then(|msg| msg.content.as_deref())
        .unwrap_or("");
    assert!(system_prompt.contains("VFS rule: only visible through injected fs."));

    let readme = engine.snapshot().await;
    assert_eq!(readme.runtime, RuntimeTarget::Wasi);

    let tool_output = register_wasi_tools_with_filesystem(workspace_fs)
        .execute("file_read", json!({ "path": "project/readme.md" }))
        .await
        .expect("file_read should use the injected filesystem");
    assert!(tool_output.contains("hello from virtual workspace"));
}
