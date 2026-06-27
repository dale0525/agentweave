use anyhow::{Context, bail};
use std::ffi::OsString;
use std::io::ErrorKind;
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

pub fn resolve_existing_workspace_path(
    root: impl AsRef<Path>,
    requested: impl AsRef<Path>,
) -> anyhow::Result<WorkspacePath> {
    let root = root.as_ref();
    let workspace_path = resolve_workspace_path(root, requested)?;
    let canonical_root = canonical_workspace_root(root)?;
    let canonical_absolute =
        ensure_existing_path_inside_canonical_workspace(&canonical_root, &workspace_path.absolute)?;

    workspace_path_from_absolute(&canonical_root, canonical_absolute)
}

pub fn resolve_workspace_output_path(
    root: impl AsRef<Path>,
    requested: impl AsRef<Path>,
) -> anyhow::Result<WorkspacePath> {
    let root = root.as_ref();
    let workspace_path = resolve_workspace_path(root, requested)?;
    let canonical_root = canonical_workspace_root(root)?;
    let (existing_ancestor, missing_suffix) = nearest_existing_ancestor(&workspace_path.absolute)?;
    let canonical_ancestor =
        ensure_existing_path_inside_canonical_workspace(&canonical_root, &existing_ancestor)?;

    workspace_path_from_absolute(&canonical_root, canonical_ancestor.join(missing_suffix))
}

pub fn ensure_existing_path_inside_workspace(
    root: impl AsRef<Path>,
    absolute: impl AsRef<Path>,
) -> anyhow::Result<PathBuf> {
    let root = root.as_ref();
    let absolute = absolute.as_ref();
    if !absolute.is_absolute() {
        bail!(
            "workspace path must be an absolute path: {}",
            absolute.display()
        );
    }

    let canonical_root = canonical_workspace_root(root)?;
    ensure_existing_path_inside_canonical_workspace(&canonical_root, absolute)
}

fn canonical_workspace_root(root: &Path) -> anyhow::Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("failed to resolve workspace root {}", root.display()))
}

fn ensure_existing_path_inside_canonical_workspace(
    canonical_root: &Path,
    absolute: &Path,
) -> anyhow::Result<PathBuf> {
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

fn workspace_path_from_absolute(
    canonical_root: &Path,
    absolute: PathBuf,
) -> anyhow::Result<WorkspacePath> {
    if !absolute.starts_with(canonical_root) {
        bail!(
            "workspace path is outside workspace: {}",
            absolute.display()
        );
    }

    let relative = absolute
        .strip_prefix(canonical_root)
        .with_context(|| format!("failed to relativize workspace path {}", absolute.display()))?
        .to_path_buf();

    Ok(WorkspacePath { relative, absolute })
}

fn nearest_existing_ancestor(path: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let mut candidate = path.to_path_buf();
    let mut missing_components = Vec::new();

    loop {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let mut missing_suffix = PathBuf::new();
                for component in missing_components.iter().rev() {
                    missing_suffix.push(component);
                }
                return Ok((candidate, missing_suffix));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let file_name = candidate.file_name().map(OsString::from).ok_or_else(|| {
                    anyhow::anyhow!("workspace output path has no existing ancestor")
                })?;
                missing_components.push(file_name);
                if !candidate.pop() {
                    bail!(
                        "workspace output path has no existing ancestor: {}",
                        path.display()
                    );
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect workspace path {}", candidate.display())
                });
            }
        }
    }
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

    #[test]
    fn resolves_existing_workspace_path_inside_workspace() {
        let root = unique_test_dir("resolve-existing-inside");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("lib.rs"), "mod tests;").unwrap();
        let canonical_root = root.canonicalize().unwrap();

        let workspace_path = resolve_existing_workspace_path(&root, "src/lib.rs").unwrap();

        assert_eq!(workspace_path.relative, PathBuf::from("src").join("lib.rs"));
        assert_eq!(
            workspace_path.absolute,
            canonical_root.join("src").join("lib.rs")
        );

        remove_test_dir(root);
    }

    #[test]
    fn resolves_workspace_output_path_inside_workspace() {
        let root = unique_test_dir("resolve-output-inside");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let canonical_root = root.canonicalize().unwrap();

        let workspace_path = resolve_workspace_output_path(&root, "src/new.rs").unwrap();

        assert_eq!(workspace_path.relative, PathBuf::from("src").join("new.rs"));
        assert_eq!(
            workspace_path.absolute,
            canonical_root.join("src").join("new.rs")
        );

        remove_test_dir(root);
    }

    #[test]
    fn rejects_relative_existing_path_check() {
        let root = unique_test_dir("reject-relative-existing-check");
        std::fs::create_dir_all(&root).unwrap();

        let error =
            ensure_existing_path_inside_workspace(&root, PathBuf::from("relative/file.txt"))
                .unwrap_err();

        assert!(error.to_string().contains("absolute path"));

        remove_test_dir(root);
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

    #[cfg(unix)]
    #[test]
    fn rejects_existing_workspace_path_with_symlink_dir_escape() {
        let root = unique_test_dir("reject-existing-symlink-dir-root");
        let outside = unique_test_dir("reject-existing-symlink-dir-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let error = resolve_existing_workspace_path(&root, "link/secret.txt").unwrap_err();

        assert!(error.to_string().contains("outside workspace"));

        remove_test_dir(root);
        remove_test_dir(outside);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_workspace_output_path_with_symlink_parent_escape() {
        let root = unique_test_dir("reject-output-symlink-parent-root");
        let outside = unique_test_dir("reject-output-symlink-parent-outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let error = resolve_workspace_output_path(&root, "link/new.txt").unwrap_err();

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
