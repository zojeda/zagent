use std::sync::Arc;

use zagent_core::provider::HttpClient;
use zagent_core::session::SessionStore;
use zagent_core::tools::ToolRegistry;

use crate::engine::RuntimeTarget;
use crate::mcp::McpManager;
use crate::platform::NativeHttpClient;
use crate::session_store::SurrealSessionStore;
use crate::session_store_json::JsonSessionStore;
use crate::tools;

pub struct RuntimeBundle {
    pub target: RuntimeTarget,
    pub http_client: Arc<dyn HttpClient>,
    pub session_store: Arc<dyn SessionStore>,
    pub session_event_store: Option<Arc<SurrealSessionStore>>,
    pub tools: Arc<ToolRegistry>,
    pub mcp_manager: Option<Arc<McpManager>>,
}

pub async fn build_runtime(
    target: RuntimeTarget,
    session_dir: &str,
    working_dir: &str,
    mcp_manager: Option<Arc<McpManager>>,
) -> Result<RuntimeBundle, zagent_core::Error> {
    match target {
        RuntimeTarget::Native => {
            let endpoint = resolve_surreal_endpoint(session_dir);
            let store = Arc::new(SurrealSessionStore::new(&endpoint).await?);
            let tools = tools::register_all_tools(working_dir, mcp_manager.clone()).await;
            Ok(RuntimeBundle {
                target,
                http_client: Arc::new(NativeHttpClient::new()),
                session_store: store.clone(),
                session_event_store: Some(store),
                tools: Arc::new(tools),
                mcp_manager,
            })
        }
        RuntimeTarget::Wasi => {
            // WASI mode currently uses a JSON session store and a restricted tool registry.
            let store = JsonSessionStore::new(format!("{session_dir}/wasi-sessions.json"))?;
            let tools = tools::register_wasi_tools();
            Ok(RuntimeBundle {
                target,
                http_client: Arc::new(NativeHttpClient::new()),
                session_store: Arc::new(store),
                session_event_store: None,
                tools: Arc::new(tools),
                mcp_manager: None,
            })
        }
    }
}

fn resolve_surreal_endpoint(session_dir_or_endpoint: &str) -> String {
    if session_dir_or_endpoint.contains("://") {
        return session_dir_or_endpoint.to_string();
    }
    std::env::var("SURREALDB_URL").unwrap_or_else(|_| "ws://127.0.0.1:8000".to_string())
}
