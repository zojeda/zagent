use std::collections::HashMap;

use tokio::sync::RwLock;
use zagent_core::Result;
use zagent_core::session::{SessionEvent, SessionMeta, SessionState, SessionStore};

/// Ephemeral in-memory session store used when SurrealDB is unavailable.
#[derive(Default)]
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, SessionState>>,
    events: RwLock<HashMap<String, Vec<SessionEvent>>>,
}

#[async_trait::async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save_session(&self, session: &SessionState) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.meta.id.clone(), session.clone());
        Ok(())
    }

    async fn load_session(&self, id: &str) -> Result<SessionState> {
        let sessions = self.sessions.read().await;
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| zagent_core::Error::session(format!("Session '{id}' not found")))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionMeta>> {
        let sessions = self.sessions.read().await;
        let mut metas: Vec<SessionMeta> = sessions.values().map(|s| s.meta.clone()).collect();
        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(metas)
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        self.sessions.write().await.remove(id);
        self.events.write().await.remove(id);
        Ok(())
    }

    async fn find_session_by_name(&self, name: &str) -> Result<Option<SessionState>> {
        let sessions = self.sessions.read().await;
        Ok(sessions.values().find(|s| s.meta.name == name).cloned())
    }

    async fn append_event(&self, event: &SessionEvent) -> Result<()> {
        let mut events = self.events.write().await;
        events
            .entry(event.session_id.clone())
            .or_default()
            .push(event.clone());
        Ok(())
    }

    async fn list_events(
        &self,
        session_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<Vec<SessionEvent>> {
        let events = self.events.read().await;
        let mut session_events = events.get(session_id).cloned().unwrap_or_default();
        if let Some(after_sequence) = after_sequence {
            session_events.retain(|event| event.sequence.unwrap_or(0) > after_sequence);
        }
        session_events.sort_by_key(|event| event.sequence.unwrap_or(0));
        Ok(session_events)
    }
}

#[cfg(test)]
mod tests {
    use zagent_core::session::SessionEvent;

    use super::*;

    #[tokio::test]
    async fn stores_sessions_and_events() {
        let store = InMemorySessionStore::default();
        let session = SessionState::new("demo", "gpt-test", "openai", "system", "/tmp");
        let session_id = session.meta.id.clone();

        store.save_session(&session).await.unwrap();
        let loaded = store.load_session(&session_id).await.unwrap();
        assert_eq!(loaded.meta.name, "demo");

        let event = SessionEvent::new(
            session_id.clone(),
            "tool_call_started",
            "main",
            0,
            Some(1),
            serde_json::json!({ "tool": "echo" }),
        );
        store.append_event(&event).await.unwrap();

        let events = store.list_events(&session_id, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "tool_call_started");
    }

    #[tokio::test]
    async fn deleting_session_clears_events() {
        let store = InMemorySessionStore::default();
        let session = SessionState::new("demo", "gpt-test", "openai", "system", "/tmp");
        let session_id = session.meta.id.clone();

        store.save_session(&session).await.unwrap();
        store
            .append_event(&SessionEvent::new(
                session_id.clone(),
                "checkpoint",
                "main",
                0,
                Some(1),
                serde_json::json!({}),
            ))
            .await
            .unwrap();

        store.delete_session(&session_id).await.unwrap();

        assert!(store.load_session(&session_id).await.is_err());
        assert!(
            store
                .list_events(&session_id, None)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
