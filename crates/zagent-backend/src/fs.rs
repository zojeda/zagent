use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use path_jail::Jail;
use tokio::fs;
use zagent_core::Result;
use zagent_core::fs::{AgentFileSystem, FileSystemEntry};
pub use zagent_loop::MemoryFileSystem;

pub type SharedFileSystem = Arc<dyn AgentFileSystem>;

#[derive(Clone, Debug)]
pub struct RootedHostFileSystem {
    jail: Jail,
    root: PathBuf,
}

impl RootedHostFileSystem {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let jail = Jail::new(root.as_ref())
            .map_err(|e| zagent_core::Error::custom(format!("Invalid filesystem root: {e}")))?;
        let root = jail.root().to_path_buf();

        Ok(Self { jail, root })
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let requested = Path::new(path);

        let candidate = if requested.is_absolute() {
            let relative = requested.strip_prefix(&self.root).map_err(|_| {
                zagent_core::Error::custom(format!(
                    "Path '{path}' escapes the configured root '{}'",
                    self.root.display()
                ))
            })?;
            self.jail.join(relative).map_err(|e| {
                zagent_core::Error::custom(format!(
                    "Path '{path}' escapes the configured root '{}': {e}",
                    self.root.display()
                ))
            })?
        } else {
            self.jail.join(requested).map_err(|e| {
                zagent_core::Error::custom(format!(
                    "Path '{path}' escapes the configured root '{}': {e}",
                    self.root.display()
                ))
            })?
        };

        Ok(candidate)
    }
}

#[async_trait]
impl AgentFileSystem for RootedHostFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String> {
        let resolved = self.resolve_path(path)?;
        fs::read_to_string(&resolved).await.map_err(Into::into)
    }

    async fn write_string(&self, path: &str, content: &str) -> Result<()> {
        let resolved = self.resolve_path(path)?;
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(resolved, content).await?;
        Ok(())
    }

    async fn list_dir(
        &self,
        path: &str,
        recursive: bool,
        max_depth: usize,
    ) -> Result<Vec<FileSystemEntry>> {
        let resolved = self.resolve_path(path)?;
        let mut out = Vec::new();
        collect_host_entries(&resolved, PathBuf::new(), &mut out, recursive, 0, max_depth).await?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

fn collect_host_entries<'a>(
    dir: &'a Path,
    relative: PathBuf,
    out: &'a mut Vec<FileSystemEntry>,
    recursive: bool,
    depth: usize,
    max_depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let metadata = fs::metadata(dir).await?;
        if !metadata.is_dir() {
            return Err(zagent_core::Error::custom(format!(
                "'{}' is not a directory",
                dir.display()
            )));
        }

        let mut reader = fs::read_dir(dir).await?;
        while let Some(entry) = reader.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            let child_relative = if relative.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                relative.join(&name)
            };
            let metadata = entry.metadata().await?;
            let relative_path = child_relative.to_string_lossy().replace('\\', "/");

            out.push(FileSystemEntry {
                path: relative_path.clone(),
                name: name.clone(),
                is_dir: metadata.is_dir(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
                depth,
            });

            if metadata.is_dir() && recursive && depth < max_depth {
                collect_host_entries(
                    &entry.path(),
                    child_relative,
                    out,
                    recursive,
                    depth + 1,
                    max_depth,
                )
                .await?;
            }
        }
        Ok(())
    })
}
