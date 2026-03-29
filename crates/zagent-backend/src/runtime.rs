use std::sync::Arc;

use zagent_core::provider::HttpClient;
use zagent_core::session::SessionStore;
use zagent_core::tools::ToolRegistry;

use crate::engine::{RuntimeTarget, SessionStoreTarget};
use crate::fs::SharedFileSystem;
use crate::mcp::McpManager;
use crate::platform::NativeHttpClient;
use crate::session_store::SurrealSessionStore;
use crate::session_store_json::JsonSessionStore;
use crate::session_store_memory::InMemorySessionStore;
use crate::tools;

pub struct RuntimeBundle {
    pub target: RuntimeTarget,
    pub http_client: Arc<dyn HttpClient>,
    pub session_store: Arc<dyn SessionStore>,
    pub session_event_store: Option<Arc<dyn SessionStore>>,
    pub session_admin_store: Option<Arc<SurrealSessionStore>>,
    pub tools: Arc<ToolRegistry>,
    pub workspace_fs: Option<SharedFileSystem>,
    pub mcp_manager: Option<Arc<McpManager>>,
}

pub async fn build_runtime(
    target: RuntimeTarget,
    session_store: SessionStoreTarget,
    session_dir: &str,
    working_dir: &str,
    mcp_manager: Option<Arc<McpManager>>,
) -> Result<RuntimeBundle, zagent_core::Error> {
    let workspace_fs: SharedFileSystem =
        Arc::new(crate::fs::RootedHostFileSystem::new(working_dir)?);
    build_runtime_with_filesystem(
        target,
        session_store,
        session_dir,
        working_dir,
        workspace_fs,
        mcp_manager,
    )
    .await
}

pub async fn build_runtime_with_filesystem(
    target: RuntimeTarget,
    session_store: SessionStoreTarget,
    session_dir: &str,
    working_dir: &str,
    workspace_fs: SharedFileSystem,
    mcp_manager: Option<Arc<McpManager>>,
) -> Result<RuntimeBundle, zagent_core::Error> {
    let http_client = Arc::new(NativeHttpClient::new());
    match target {
        RuntimeTarget::Native => {
            let tools = tools::register_all_tools_with_filesystem(
                workspace_fs.clone(),
                working_dir,
                mcp_manager.clone(),
            )
            .await;
            match session_store {
                SessionStoreTarget::Surreal => {
                    let endpoint = resolve_surreal_endpoint(session_dir);
                    let store = Arc::new(SurrealSessionStore::new(&endpoint).await?);
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: store.clone(),
                        session_event_store: Some(store.clone()),
                        session_admin_store: Some(store),
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager,
                    })
                }
                SessionStoreTarget::Memory => {
                    let store = Arc::new(InMemorySessionStore::default());
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: store.clone(),
                        session_event_store: Some(store),
                        session_admin_store: None,
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager,
                    })
                }
                SessionStoreTarget::Json => {
                    let store =
                        JsonSessionStore::new(format!("{session_dir}/native-sessions.json"))?;
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: Arc::new(store),
                        session_event_store: None,
                        session_admin_store: None,
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager,
                    })
                }
            }
        }
        RuntimeTarget::Wasi => {
            let tools = tools::register_wasi_tools_with_filesystem(workspace_fs.clone());
            match session_store {
                SessionStoreTarget::Surreal => {
                    let endpoint = resolve_surreal_endpoint(session_dir);
                    let store = Arc::new(SurrealSessionStore::new(&endpoint).await?);
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: store.clone(),
                        session_event_store: Some(store.clone()),
                        session_admin_store: Some(store),
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager: None,
                    })
                }
                SessionStoreTarget::Json => {
                    let store = JsonSessionStore::new(format!("{session_dir}/wasi-sessions.json"))?;
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: Arc::new(store),
                        session_event_store: None,
                        session_admin_store: None,
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager: None,
                    })
                }
                SessionStoreTarget::Memory => {
                    let store = Arc::new(InMemorySessionStore::default());
                    Ok(RuntimeBundle {
                        target,
                        http_client,
                        session_store: store.clone(),
                        session_event_store: Some(store),
                        session_admin_store: None,
                        tools: Arc::new(tools),
                        workspace_fs: Some(workspace_fs),
                        mcp_manager: None,
                    })
                }
            }
        }
    }
}

fn resolve_surreal_endpoint(session_dir_or_endpoint: &str) -> String {
    if session_dir_or_endpoint.contains("://") {
        return session_dir_or_endpoint.to_string();
    }
    std::env::var("SURREALDB_URL").unwrap_or_else(|_| "ws://127.0.0.1:8000".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zagent_core::session::SessionState;

    #[tokio::test]
    async fn native_runtime_uses_in_memory_store_when_selected() {
        let runtime = build_runtime(
            RuntimeTarget::Native,
            SessionStoreTarget::Memory,
            "ws://127.0.0.1:1",
            "/tmp",
            None,
        )
        .await
        .unwrap();

        assert!(runtime.session_admin_store.is_none());
        assert!(runtime.session_event_store.is_some());

        let session = SessionState::new("fallback", "gpt-test", "openai", "system", "/tmp");
        let session_id = session.meta.id.clone();
        runtime.session_store.save_session(&session).await.unwrap();

        let loaded = runtime
            .session_store
            .load_session(&session_id)
            .await
            .unwrap();
        assert_eq!(loaded.meta.name, "fallback");
    }
}
