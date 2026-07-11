use crate::skill_store_secure_fs::prepare_directory_path;
use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) async fn prepare_canonical_directory(path: &Path) -> anyhow::Result<PathBuf> {
    if let Ok(canonical) = tokio::fs::canonicalize(path).await {
        return Ok(canonical);
    }
    let mut current = path;
    let mut missing = Vec::new();
    let canonical_ancestor = loop {
        if let Ok(canonical) = tokio::fs::canonicalize(current).await {
            break canonical;
        }
        missing.push(
            current
                .file_name()
                .context("skill store path has no existing ancestor")?
                .to_os_string(),
        );
        current = current
            .parent()
            .context("skill store path has no existing ancestor")?;
    };
    let mut prepared = canonical_ancestor;
    for component in missing.into_iter().rev() {
        prepared.push(component);
    }
    prepare_directory_path(&prepared).await?;
    Ok(tokio::fs::canonicalize(prepared).await?)
}
