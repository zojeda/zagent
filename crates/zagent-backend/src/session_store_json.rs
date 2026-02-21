use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;
use zagent_core::Result;
use zagent_core::session::{SessionMeta, SessionState, SessionStore};

/// JSON-backed session store used by the WASI runtime mode.
pub struct JsonSessionStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl JsonSessionStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                zagent_core::Error::session(format!("Failed to create session dir: {e}"))
            })?;
        }

        if !path.exists() {
            std::fs::write(&path, "{}").map_err(|e| {
                zagent_core::Error::session(format!("Failed to init session store: {e}"))
            })?;
        }

        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    async fn load_all(&self) -> Result<HashMap<String, SessionState>> {
        let raw = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to read sessions: {e}")))?;

        if raw.trim().is_empty() {
            return Ok(HashMap::new());
        }

        serde_json::from_str(&raw)
            .map_err(|e| zagent_core::Error::session(format!("Invalid session JSON: {e}")))
    }

    async fn save_all(&self, sessions: &HashMap<String, SessionState>) -> Result<()> {
        let raw = serde_json::to_string_pretty(sessions)
            .map_err(|e| zagent_core::Error::session(format!("Failed to encode sessions: {e}")))?;

        tokio::fs::write(&self.path, raw)
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to write sessions: {e}")))?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionStore for JsonSessionStore {
    async fn save_session(&self, session: &SessionState) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut sessions = self.load_all().await?;
        sessions.insert(session.meta.id.clone(), session.clone());
        self.save_all(&sessions).await
    }

    async fn load_session(&self, id: &str) -> Result<SessionState> {
        let _guard = self.lock.lock().await;
        let sessions = self.load_all().await?;
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| zagent_core::Error::session(format!("Session '{id}' not found")))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let _guard = self.lock.lock().await;
        let sessions = self.load_all().await?;
        let mut metas: Vec<SessionMeta> = sessions.values().map(|s| s.meta.clone()).collect();
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut sessions = self.load_all().await?;
        sessions.remove(id);
        self.save_all(&sessions).await
    }

    async fn find_session_by_name(&self, name: &str) -> Result<Option<SessionState>> {
        let _guard = self.lock.lock().await;
        let sessions = self.load_all().await?;
        Ok(sessions.values().find(|s| s.meta.name == name).cloned())
    }
}
