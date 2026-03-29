use std::collections::HashMap;
use std::sync::Arc;

use zagent_core::Result;
use zagent_core::agent::{
    AgentConfig, AgentProgressEvent, ContextManagementPolicy, run_agent_loop_with_progress,
};
use zagent_core::fs::AgentFileSystem;
use zagent_core::provider::{HttpClient, Provider, StaticProviderResolver};
use zagent_core::session::{SessionState, SessionStore};
use zagent_core::tools::ToolRegistry;
use tokio::sync::Mutex;

use crate::session::InMemorySessionStore;
use crate::tools::build_file_tools;

#[derive(Debug, Clone)]
pub struct LoopAgentOptions {
    pub provider_name: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub session_name: String,
    pub working_dir: String,
    pub max_turns: u32,
    pub context_management_policy: ContextManagementPolicy,
}

impl LoopAgentOptions {
    pub fn new(
        provider_name: impl Into<String>,
        model: impl Into<String>,
        session_name: impl Into<String>,
        working_dir: impl Into<String>,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            model: model.into(),
            system_prompt: None,
            session_name: session_name.into(),
            working_dir: working_dir.into(),
            max_turns: 50,
            context_management_policy: ContextManagementPolicy::default(),
        }
    }
}

impl Default for LoopAgentOptions {
    fn default() -> Self {
        Self::new("openai", "gpt-5.2", "loop", ".")
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoopAgentResponse {
    pub output: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub struct LoopAgent {
    http_client: Arc<dyn HttpClient>,
    providers: HashMap<String, Arc<dyn Provider>>,
    provider_name: String,
    tools: Arc<ToolRegistry>,
    workspace_fs: Arc<dyn AgentFileSystem>,
    session_store: Arc<dyn SessionStore>,
    config: AgentConfig,
    session: Mutex<SessionState>,
}

impl LoopAgent {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        providers: HashMap<String, Arc<dyn Provider>>,
        workspace_fs: Arc<dyn AgentFileSystem>,
        options: LoopAgentOptions,
    ) -> Result<Self> {
        if providers.is_empty() {
            return Err(zagent_core::Error::config(
                "Loop agent requires at least one configured provider",
            ));
        }
        if !providers.contains_key(&options.provider_name) {
            return Err(zagent_core::Error::config(format!(
                "Provider '{}' is not configured",
                options.provider_name
            )));
        }

        let config = AgentConfig {
            model: options.model.clone(),
            custom_agent_default_model: options.model.clone(),
            max_turns: options.max_turns,
            system_prompt: options
                .system_prompt
                .unwrap_or_else(|| AgentConfig::default().system_prompt),
            context_management_policy: options.context_management_policy,
            ..AgentConfig::default()
        };

        let session = SessionState::new(
            options.session_name,
            options.model,
            options.provider_name.clone(),
            config.system_prompt.clone(),
            options.working_dir,
        );

        Ok(Self {
            http_client,
            providers,
            provider_name: options.provider_name,
            tools: Arc::new(build_file_tools(workspace_fs.clone())),
            workspace_fs,
            session_store: Arc::new(InMemorySessionStore::default()),
            config,
            session: Mutex::new(session),
        })
    }

    pub async fn send_input(&self, input: &str) -> Result<LoopAgentResponse> {
        self.send_input_with_progress(input, |_| {}).await
    }

    pub async fn send_input_with_progress<F>(
        &self,
        input: &str,
        mut progress: F,
    ) -> Result<LoopAgentResponse>
    where
        F: FnMut(AgentProgressEvent) + Send,
    {
        let provider_resolver = StaticProviderResolver::new(&self.provider_name, &self.providers);
        let mut session = self.session.lock().await;
        let result = run_agent_loop_with_progress(
            self.http_client.as_ref(),
            &provider_resolver,
            self.tools.as_ref(),
            Some(self.workspace_fs.as_ref()),
            &mut session,
            Some(self.session_store.as_ref()),
            &self.config,
            input,
            Some(&mut progress),
        )
        .await?;

        Ok(LoopAgentResponse {
            output: result.response,
            turns: result.turns,
            tool_calls: result.tool_calls_count,
            prompt_tokens: result.total_prompt_tokens,
            completion_tokens: result.total_completion_tokens,
        })
    }

    pub fn tool_names(&self) -> Vec<&str> {
        self.tools.tool_names()
    }
}
