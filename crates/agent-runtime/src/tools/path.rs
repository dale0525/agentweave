use anyhow::{Context, bail};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePath {
    pub relative: PathBuf,
    pub absolute: PathBuf,
}

pub fn resolve_workspace_path(
    root: impl AsRef<Path>,
    requested: impl AsRef<Path>,
) -> anyhow::Result<WorkspacePath> {
    let root = root.as_ref();
    let requested = requested.as_ref();
    if requested.as_os_str().is_empty() {
        bail!("empty workspace path");
    }

    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace root {}", root.display()))?;
    let relative = if requested.is_absolute() {
        let normalized_absolute = normalize_absolute_path(requested)?;
        if !normalized_absolute.starts_with(&canonical_root) {
            bail!(
                "workspace path is outside workspace: {}",
                requested.display()
            );
        }
        let stripped = normalized_absolute
            .strip_prefix(&canonical_root)
            .with_context(|| {
                format!(
                    "failed to relativize workspace path {}",
                    requested.display()
                )
            })?;
        normalize_relative_path(stripped)?
    } else {
        normalize_relative_path(requested)?
    };

    Ok(WorkspacePath {
        absolute: canonical_root.join(&relative),
        relative,
    })
}

pub fn ensure_existing_path_inside_workspace(
    root: impl AsRef<Path>,
    absolute: impl AsRef<Path>,
) -> anyhow::Result<PathBuf> {
    let root = root.as_ref();
    let absolute = absolute.as_ref();
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace root {}", root.display()))?;
    let canonical_path = absolute
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace path {}", absolute.display()))?;

    if !canonical_path.starts_with(&canonical_root) {
        bail!(
            "workspace path is outside workspace: {}",
            absolute.display()
        );
    }

    Ok(canonical_path)
}

fn normalize_relative_path(path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                bail!("parent traversal is not allowed: {}", path.display());
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "absolute path cannot be normalized as relative: {}",
                    path.display()
                );
            }
        }
    }
    Ok(normalized)
}

fn normalize_absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                bail!("parent traversal is not allowed: {}", path.display());
            }
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolves_relative_path_inside_workspace() {
        let root = unique_test_dir("resolve-relative");
        std::fs::create_dir_all(&root).unwrap();
        let canonical_root = root.canonicalize().unwrap();

        let workspace_path = resolve_workspace_path(&root, "src/./lib.rs").unwrap();
        let root_path = resolve_workspace_path(&root, ".").unwrap();

        assert_eq!(workspace_path.relative, PathBuf::from("src").join("lib.rs"));
        assert_eq!(
            workspace_path.absolute,
            canonical_root.join("src").join("lib.rs")
        );
        assert_eq!(root_path.relative, PathBuf::new());
        assert_eq!(root_path.absolute, canonical_root);

        remove_test_dir(root);
    }

    #[test]
    fn rejects_parent_traversal() {
        let root = unique_test_dir("reject-parent");
        std::fs::create_dir_all(&root).unwrap();

        let direct_parent = resolve_workspace_path(&root, "../secret").unwrap_err();
        let nested_parent = resolve_workspace_path(&root, "src/../secret").unwrap_err();

        assert!(direct_parent.to_string().contains("parent traversal"));
        assert!(nested_parent.to_string().contains("parent traversal"));

        remove_test_dir(root);
    }

    #[test]
    fn rejects_empty_path() {
        let root = unique_test_dir("reject-empty");
        std::fs::create_dir_all(&root).unwrap();

        let error = resolve_workspace_path(&root, "").unwrap_err();

        assert!(error.to_string().contains("empty workspace path"));

        remove_test_dir(root);
    }

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let root = unique_test_dir("reject-absolute-root");
        let outside = unique_test_dir("reject-absolute-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let error = resolve_workspace_path(&root, outside.join("secret.txt")).unwrap_err();

        assert!(error.to_string().contains("outside workspace"));

        remove_test_dir(root);
        remove_test_dir(outside);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_symlink_that_escapes_workspace() {
        let root = unique_test_dir("reject-symlink-root");
        let outside = unique_test_dir("reject-symlink-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("secret.txt");
        std::fs::write(&outside_file, "secret").unwrap();
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();

        let error = ensure_existing_path_inside_workspace(&root, &link).unwrap_err();

        assert!(error.to_string().contains("outside workspace"));

        remove_test_dir(root);
        remove_test_dir(outside);
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
