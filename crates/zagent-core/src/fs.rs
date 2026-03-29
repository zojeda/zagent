use async_trait::async_trait;

use crate::Result;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileSystemEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub depth: usize,
}

#[async_trait]
pub trait AgentFileSystem: Send + Sync {
    async fn read_to_string(&self, path: &str) -> Result<String>;
    async fn write_string(&self, path: &str, content: &str) -> Result<()>;
    async fn list_dir(
        &self,
        path: &str,
        recursive: bool,
        max_depth: usize,
    ) -> Result<Vec<FileSystemEntry>>;
}
