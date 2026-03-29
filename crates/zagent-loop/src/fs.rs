use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs;
use tokio::sync::RwLock;
use zagent_core::Result;
use zagent_core::fs::{AgentFileSystem, FileSystemEntry};

#[derive(Clone, Debug)]
pub struct HostFileSystem;

#[async_trait]
impl AgentFileSystem for HostFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String> {
        fs::read_to_string(path).await.map_err(Into::into)
    }

    async fn write_string(&self, path: &str, content: &str) -> Result<()> {
        let resolved = Path::new(path);
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
        let resolved = Path::new(path);
        let mut out = Vec::new();
        collect_host_entries(resolved, PathBuf::new(), &mut out, recursive, 0, max_depth).await?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemoryFileSystem {
    inner: Arc<RwLock<MemoryState>>,
}

#[derive(Debug, Default)]
struct MemoryState {
    files: BTreeMap<String, String>,
    dirs: BTreeSet<String>,
}

impl MemoryFileSystem {
    pub fn new() -> Self {
        Self::from_state(MemoryState {
            files: BTreeMap::new(),
            dirs: BTreeSet::from([String::new()]),
        })
    }

    pub fn from_iter<I, P, C>(files: I) -> Self
    where
        I: IntoIterator<Item = (P, C)>,
        P: AsRef<str>,
        C: AsRef<str>,
    {
        let mut state = MemoryState {
            files: BTreeMap::new(),
            dirs: BTreeSet::from([String::new()]),
        };
        for (path, content) in files {
            let normalized =
                normalize_virtual_path(path.as_ref()).expect("valid virtual file path");
            state.insert_file(normalized, content.as_ref().to_string());
        }
        Self::from_state(state)
    }

    pub fn from_files<const N: usize>(files: [(&str, &str); N]) -> Self {
        Self::from_iter(files)
    }

    fn from_state(state: MemoryState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(state)),
        }
    }
}

impl MemoryState {
    fn insert_file(&mut self, path: String, content: String) {
        self.ensure_parent_dirs(&path);
        self.files.insert(path, content);
    }

    fn ensure_parent_dirs(&mut self, path: &str) {
        self.dirs.insert(String::new());
        let mut current = Vec::new();
        if let Some(parent) = Path::new(path).parent() {
            for component in parent.components() {
                if let Component::Normal(part) = component {
                    current.push(part.to_string_lossy().to_string());
                    self.dirs.insert(current.join("/"));
                }
            }
        }
    }
}

#[async_trait]
impl AgentFileSystem for MemoryFileSystem {
    async fn read_to_string(&self, path: &str) -> Result<String> {
        let normalized = normalize_virtual_path(path)?;
        let state = self.inner.read().await;
        state.files.get(&normalized).cloned().ok_or_else(|| {
            zagent_core::Error::custom(format!(
                "Path '{path}' does not exist in the virtual filesystem"
            ))
        })
    }

    async fn write_string(&self, path: &str, content: &str) -> Result<()> {
        let normalized = normalize_virtual_path(path)?;
        let mut state = self.inner.write().await;
        state.insert_file(normalized, content.to_string());
        Ok(())
    }

    async fn list_dir(
        &self,
        path: &str,
        recursive: bool,
        max_depth: usize,
    ) -> Result<Vec<FileSystemEntry>> {
        let normalized = normalize_virtual_dir_path(path)?;
        let state = self.inner.read().await;

        if state.files.contains_key(&normalized) {
            return Err(zagent_core::Error::custom(format!(
                "'{path}' is not a directory"
            )));
        }

        let prefix = if normalized.is_empty() {
            String::new()
        } else {
            format!("{normalized}/")
        };

        let dir_exists = normalized.is_empty()
            || state.dirs.contains(&normalized)
            || state.files.keys().any(|file| file.starts_with(&prefix));
        if !dir_exists {
            return Err(zagent_core::Error::custom(format!(
                "Path '{path}' does not exist"
            )));
        }

        let mut out = Vec::new();
        let mut seen_dirs = BTreeSet::new();

        for dir in &state.dirs {
            if dir.is_empty() || dir == &normalized || !dir.starts_with(&prefix) {
                continue;
            }
            let relative = dir.strip_prefix(&prefix).unwrap_or(dir);
            let depth = relative.matches('/').count();
            if (!recursive && depth > 0) || depth > max_depth {
                continue;
            }
            if seen_dirs.insert(relative.to_string()) {
                out.push(entry_from_relative(relative, true, 0));
            }
        }

        for (file_path, content) in &state.files {
            if !file_path.starts_with(&prefix) {
                continue;
            }
            let relative = file_path.strip_prefix(&prefix).unwrap_or(file_path);
            let depth = relative.matches('/').count();
            if (!recursive && depth > 0) || depth > max_depth {
                continue;
            }
            out.push(entry_from_relative(relative, false, content.len() as u64));
        }

        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

fn entry_from_relative(relative: &str, is_dir: bool, size: u64) -> FileSystemEntry {
    let name = Path::new(relative)
        .file_name()
        .map(|part| part.to_string_lossy().to_string())
        .unwrap_or_else(|| relative.to_string());
    FileSystemEntry {
        path: relative.to_string(),
        name,
        is_dir,
        size,
        depth: relative.matches('/').count(),
    }
}

fn normalize_virtual_path(path: &str) -> Result<String> {
    let normalized = normalize_relative_components(path)?;
    if normalized.is_empty() {
        return Err(zagent_core::Error::custom(
            "Virtual filesystem paths must point to a file",
        ));
    }
    Ok(normalized)
}

fn normalize_virtual_dir_path(path: &str) -> Result<String> {
    if path.is_empty() || path == "." {
        return Ok(String::new());
    }
    normalize_relative_components(path)
}

fn normalize_relative_components(path: &str) -> Result<String> {
    let requested = Path::new(path);
    if requested.is_absolute() {
        return Err(zagent_core::Error::custom(format!(
            "Absolute paths are not allowed in the virtual filesystem: '{path}'"
        )));
    }

    let mut parts: Vec<String> = Vec::new();
    for component in requested.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(zagent_core::Error::custom(format!(
                        "Path '{path}' escapes the configured root"
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(zagent_core::Error::custom(format!(
                    "Unsupported filesystem path: '{path}'"
                )));
            }
        }
    }

    Ok(parts.join("/"))
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
                name,
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

#[cfg(test)]
mod tests {
    use super::MemoryFileSystem;
    use zagent_core::fs::AgentFileSystem;

    #[tokio::test]
    async fn memory_filesystem_supports_dynamic_iter_construction() {
        let fs = MemoryFileSystem::from_iter(vec![
            ("AGENTS.md".to_string(), "Rules".to_string()),
            ("notes/todo.txt".to_string(), "Ship wasm demo".to_string()),
        ]);

        let agents = fs.read_to_string("AGENTS.md").await.expect("agents file");
        let todo = fs
            .read_to_string("notes/todo.txt")
            .await
            .expect("todo file");

        assert_eq!(agents, "Rules");
        assert_eq!(todo, "Ship wasm demo");
    }
}
