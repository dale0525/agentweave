use serde::{Deserialize, Serialize};
use std::fs;
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
    PathEscape,
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
        let root_path = match root {
            VfsRoot::Documents => self.documents_root.join(safe_relative),
            VfsRoot::Cache => self.cache_root.join(safe_relative),
        };
        let base_root = match root {
            VfsRoot::Documents => &self.documents_root,
            VfsRoot::Cache => &self.cache_root,
        };
        ensure_existing_path_contained(base_root, &root_path)?;
        Ok(root_path)
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

fn ensure_existing_path_contained(root: &Path, resolved_path: &Path) -> Result<(), VfsError> {
    let Some(canonical_root) = canonicalize_if_exists(root)? else {
        return Ok(());
    };
    let mut current = root.to_path_buf();

    for component in resolved_path.strip_prefix(root).map_err(|_| VfsError::PathEscape)?.components()
    {
        let Component::Normal(segment) = component else {
            return Err(VfsError::PathEscape);
        };
        current.push(segment);
        if let Some(canonical_current) = canonicalize_if_exists(&current)? {
            if !path_is_contained(&canonical_root, &canonical_current) {
                return Err(VfsError::PathEscape);
            }
        }
    }

    Ok(())
}

fn canonicalize_if_exists(path: &Path) -> Result<Option<PathBuf>, VfsError> {
    match fs::symlink_metadata(path) {
        Ok(_) => fs::canonicalize(path)
            .map(Some)
            .map_err(|_| VfsError::PathEscape),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(VfsError::PathEscape),
    }
}

fn path_is_contained(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::{AppDataVfs, VfsError};
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_inside_documents_root() {
        let temp_root = make_temp_dir("vfs-symlink-escape");
        let app_root = temp_root.join("app");
        let documents_root = app_root.join("documents");
        let cache_root = app_root.join("cache");
        let outside_root = temp_root.join("outside");
        let link_path = documents_root.join("link");

        fs::create_dir_all(&documents_root).unwrap();
        fs::create_dir_all(&cache_root).unwrap();
        fs::create_dir_all(&outside_root).unwrap();
        symlink(&outside_root, &link_path).unwrap();

        let vfs = AppDataVfs::new(&documents_root, &cache_root);
        assert_eq!(
            vfs.resolve_uri("app://documents/link/secret.txt").unwrap_err(),
            VfsError::PathEscape
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlink_escape_inside_documents_root() {
        let temp_root = make_temp_dir("vfs-dangling-symlink-escape");
        let app_root = temp_root.join("app");
        let documents_root = app_root.join("documents");
        let cache_root = app_root.join("cache");
        let outside_root = temp_root.join("outside-missing");
        let link_path = documents_root.join("link");

        fs::create_dir_all(&documents_root).unwrap();
        fs::create_dir_all(&cache_root).unwrap();
        symlink(&outside_root, &link_path).unwrap();

        let vfs = AppDataVfs::new(&documents_root, &cache_root);
        assert_eq!(
            vfs.resolve_uri("app://documents/link/new.txt").unwrap_err(),
            VfsError::PathEscape
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
