use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AppDataVfs {
    documents_root: PathBuf,
    cache_root: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum VfsRoot {
    Documents,
    Cache,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum VfsError {
    UnsupportedScheme,
    UnsupportedRoot,
    EmptyPath,
    PathTraversal,
}

impl AppDataVfs {
    pub fn new(documents_root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            documents_root: documents_root.into(),
            cache_root: cache_root.into(),
        }
    }

    pub fn resolve_uri(&self, uri: &str) -> Result<PathBuf, VfsError> {
        let rest = uri
            .strip_prefix("app://")
            .ok_or(VfsError::UnsupportedScheme)?;
        let (root_name, relative) = rest.split_once('/').ok_or(VfsError::EmptyPath)?;
        let root = match root_name {
            "documents" => VfsRoot::Documents,
            "cache" => VfsRoot::Cache,
            _ => return Err(VfsError::UnsupportedRoot),
        };
        let safe_relative = safe_relative_path(relative)?;
        Ok(match root {
            VfsRoot::Documents => self.documents_root.join(safe_relative),
            VfsRoot::Cache => self.cache_root.join(safe_relative),
        })
    }
}

fn safe_relative_path(value: &str) -> Result<PathBuf, VfsError> {
    if value.trim().is_empty() {
        return Err(VfsError::EmptyPath);
    }
    let path = Path::new(value);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => result.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(VfsError::PathTraversal);
            }
        }
    }
    if result.as_os_str().is_empty() {
        return Err(VfsError::EmptyPath);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::{AppDataVfs, VfsError};
    use std::path::PathBuf;

    #[test]
    fn resolves_documents_uri_inside_app_root() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://documents/notes/today.md").unwrap(),
            PathBuf::from("/app/files/documents/notes/today.md")
        );
    }

    #[test]
    fn rejects_absolute_paths() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("/etc/passwd").unwrap_err(),
            VfsError::UnsupportedScheme
        );
    }

    #[test]
    fn rejects_traversal() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://documents/../secrets.txt").unwrap_err(),
            VfsError::PathTraversal
        );
    }

    #[test]
    fn rejects_unknown_app_root() {
        let vfs = AppDataVfs::new("/app/files/documents", "/app/files/cache");
        assert_eq!(
            vfs.resolve_uri("app://skills/SKILL.md").unwrap_err(),
            VfsError::UnsupportedRoot
        );
    }
}
