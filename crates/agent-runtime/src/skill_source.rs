use crate::skill_package::SkillPackageDescriptor;
use anyhow::Context;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillLayer {
    Builtin,
    Managed,
    Session,
}

#[derive(Clone, Debug)]
pub struct DiscoveredSkillPackage {
    pub layer: SkillLayer,
    pub root: PathBuf,
    pub descriptor: SkillPackageDescriptor,
    pub content_hash: String,
    pub warnings: Vec<String>,
}

#[async_trait]
pub trait SkillSource: Send + Sync {
    fn layer(&self) -> SkillLayer;
    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>>;
}

pub struct DirectorySkillSource {
    layer: SkillLayer,
    root: PathBuf,
}

impl DirectorySkillSource {
    pub fn new(layer: SkillLayer, root: impl Into<PathBuf>) -> Self {
        Self {
            layer,
            root: root.into(),
        }
    }
}

#[async_trait]
impl SkillSource for DirectorySkillSource {
    fn layer(&self) -> SkillLayer {
        self.layer
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        let mut roots = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.root)
            .await
            .with_context(|| format!("failed to read skill source {}", self.root.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                roots.push(entry.path());
            }
        }
        roots.sort();

        let mut packages = Vec::with_capacity(roots.len());
        let mut seen = BTreeMap::new();
        for root in roots {
            let loaded = SkillPackageDescriptor::load(&root).await?;
            loaded.descriptor.validate()?;
            if let Some(previous_root) =
                seen.insert(loaded.descriptor.id.clone(), loaded.root.clone())
            {
                anyhow::bail!(
                    "duplicate package id {} in {:?} source: {} and {}",
                    loaded.descriptor.id.as_str(),
                    self.layer,
                    previous_root.display(),
                    loaded.root.display()
                );
            }
            packages.push(DiscoveredSkillPackage {
                layer: self.layer,
                content_hash: hash_package_tree(&loaded.root).await?,
                root: loaded.root,
                descriptor: loaded.descriptor,
                warnings: loaded.warnings,
            });
        }
        packages.sort_by(|left, right| {
            left.descriptor
                .id
                .cmp(&right.descriptor.id)
                .then_with(|| left.root.cmp(&right.root))
        });
        Ok(packages)
    }
}

pub async fn hash_package_tree(root: &Path) -> anyhow::Result<String> {
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to inspect skill package root {}", root.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("skill package root cannot be a symlink: {}", root.display());
    }
    if !metadata.is_dir() {
        anyhow::bail!("skill package root must be a directory: {}", root.display());
    }

    let mut files = Vec::new();
    collect_files(root, &mut files).await?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn collect_files(root: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) -> anyhow::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read package directory {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let kind = entry.file_type().await?;
            if kind.is_symlink() {
                anyhow::bail!("skill package cannot contain symlinks: {}", path.display());
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if kind.is_file() {
                let relative = path.strip_prefix(root)?.to_path_buf();
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("failed to read package file {}", path.display()))?;
                files.push((relative, bytes));
            }
        }
    }
    Ok(())
}
