use crate::skill_source::{
    canonical_relative_path, portable_collision_key, register_portable_path,
};
use anyhow::Context;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StoreFaultPoint {
    StagingCopyFile,
    IncomingCopyFile,
    QuarantineCopyFile,
    PromoteStagingRename,
    PromoteIncomingRename,
    PromoteDatabase,
    PromoteRestoreRename,
    QuarantineIncomingRename,
    QuarantineSourceRename,
    QuarantineDatabase,
    QuarantineRestoreRename,
}

#[derive(Clone, Default)]
pub(crate) struct StoreFaults {
    failures: Arc<Mutex<BTreeMap<StoreFaultPoint, usize>>>,
}

impl StoreFaults {
    #[cfg(test)]
    pub(crate) fn fail_once(&self, point: StoreFaultPoint) {
        self.failures.lock().unwrap().insert(point, 0);
    }

    #[cfg(test)]
    pub(crate) fn fail_after(&self, point: StoreFaultPoint, successful_checks: usize) {
        self.failures
            .lock()
            .unwrap()
            .insert(point, successful_checks);
    }

    pub(crate) fn check(&self, point: StoreFaultPoint) -> anyhow::Result<()> {
        let mut failures = self.failures.lock().unwrap();
        let Some(remaining) = failures.get_mut(&point) else {
            return Ok(());
        };
        if *remaining == 0 {
            failures.remove(&point);
            anyhow::bail!("injected skill store failure at {point:?}")
        }
        *remaining -= 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PackageLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
}

#[derive(Debug)]
struct PackageEntries {
    directories: Vec<PathBuf>,
    files: Vec<PackageFile>,
}

#[derive(Debug)]
struct PackageFile {
    relative: PathBuf,
    expected_bytes: u64,
}

pub(crate) async fn measure_package_tree(
    root: &Path,
    limits: PackageLimits,
    exclude: Option<&Path>,
) -> anyhow::Result<u64> {
    let entries = collect_entries(root, limits).await?;
    let mut package_bytes = 0_u64;
    for file in entries.files {
        if exclude.is_some_and(|path| path == file.relative) {
            continue;
        }
        let bytes = read_file_size(root, &file, limits.max_file_bytes).await?;
        package_bytes = checked_package_add(package_bytes, bytes, limits.max_package_bytes)?;
    }
    Ok(package_bytes)
}

pub(crate) async fn copy_package_tree(
    source: &Path,
    destination: &Path,
    limits: PackageLimits,
    faults: &StoreFaults,
    copy_fault: StoreFaultPoint,
) -> anyhow::Result<()> {
    reject_existing(destination).await?;
    let entries = collect_entries(source, limits).await?;
    tokio::fs::create_dir(destination)
        .await
        .with_context(|| format!("failed to create package copy {}", destination.display()))?;
    for relative in &entries.directories {
        tokio::fs::create_dir(destination.join(relative))
            .await
            .with_context(|| {
                format!(
                    "failed to create package directory {}",
                    destination.join(relative).display()
                )
            })?;
    }

    let mut package_bytes = 0_u64;
    for file in entries.files {
        faults.check(copy_fault)?;
        let bytes = copy_regular_file(source, destination, &file, limits.max_file_bytes).await?;
        package_bytes = checked_package_add(package_bytes, bytes, limits.max_package_bytes)?;
    }
    Ok(())
}

pub(crate) async fn ensure_safe_write_parent(root: &Path, relative: &Path) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let root_metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to inspect staging root {}", root.display()))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        anyhow::bail!(
            "staging revision root must be a real directory: {}",
            root.display()
        );
    }
    let mut current = root.to_path_buf();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            current.push(component.as_os_str());
            match tokio::fs::symlink_metadata(&current).await {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    anyhow::bail!(
                        "staging path cannot traverse a symlink: {}",
                        current.display()
                    )
                }
                Ok(metadata) if !metadata.is_dir() => {
                    anyhow::bail!("staging parent must be a directory: {}", current.display())
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::fs::create_dir(&current).await.with_context(|| {
                        format!("failed to create staging directory {}", current.display())
                    })?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    let destination = root.join(relative);
    if let Ok(metadata) = tokio::fs::symlink_metadata(&destination).await
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        anyhow::bail!(
            "staging destination must be a regular file: {}",
            destination.display()
        );
    }
    Ok(())
}

pub(crate) async fn make_tree_readonly(root: &Path, limits: PackageLimits) -> anyhow::Result<()> {
    let entries = collect_entries(root, limits).await?;
    for file in entries.files {
        set_readonly(&root.join(file.relative), false).await?;
    }
    let mut directories = entries.directories;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for relative in directories {
        set_readonly(&root.join(relative), true).await?;
    }
    set_readonly(root, true).await
}

pub(crate) async fn make_tree_writable(root: &Path, limits: PackageLimits) -> anyhow::Result<()> {
    let entries = collect_entries(root, limits).await?;
    set_writable(root, true).await?;
    for relative in &entries.directories {
        set_writable(&root.join(relative), true).await?;
    }
    for file in entries.files {
        set_writable(&root.join(file.relative), false).await?;
    }
    Ok(())
}

async fn collect_entries(root: &Path, limits: PackageLimits) -> anyhow::Result<PackageEntries> {
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to inspect package root {}", root.display()))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("skill package root cannot be a symlink: {}", root.display());
    }
    if !metadata.is_dir() {
        anyhow::bail!("skill package root must be a directory: {}", root.display());
    }

    let mut stack = vec![PathBuf::new()];
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut portable_paths = BTreeMap::<Vec<u8>, PathBuf>::new();
    let mut metadata_total = 0_u64;
    while let Some(relative_directory) = stack.pop() {
        let directory = root.join(&relative_directory);
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read package directory {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let relative = relative_directory.join(entry.file_name());
            canonical_relative_path(&relative)?;
            let collision_key = portable_collision_key(&relative)?;
            register_portable_path(&mut portable_paths, &relative, &collision_key)?;
            let path = root.join(&relative);
            let metadata = tokio::fs::symlink_metadata(&path)
                .await
                .with_context(|| format!("failed to inspect package path {}", path.display()))?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                anyhow::bail!("skill package cannot contain symlinks: {}", path.display());
            }
            if kind.is_dir() {
                directories.push(relative.clone());
                stack.push(relative);
                continue;
            }
            if !kind.is_file() {
                anyhow::bail!(
                    "skill package cannot contain special files: {}",
                    path.display()
                );
            }
            if metadata.len() > limits.max_file_bytes {
                anyhow::bail!(
                    "skill package file exceeds {} byte limit: {}",
                    limits.max_file_bytes,
                    path.display()
                );
            }
            metadata_total =
                checked_package_add(metadata_total, metadata.len(), limits.max_package_bytes)?;
            files.push(PackageFile {
                relative,
                expected_bytes: metadata.len(),
            });
        }
    }
    directories.sort();
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(PackageEntries { directories, files })
}

async fn read_file_size(
    root: &Path,
    file: &PackageFile,
    max_file_bytes: u64,
) -> anyhow::Result<u64> {
    let path = root.join(&file.relative);
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("package path changed while reading: {}", path.display());
    }
    let mut source = tokio::fs::File::open(&path).await?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut bytes = 0_u64;
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(u64::try_from(count)?)
            .context("package file size overflow")?;
        if bytes > max_file_bytes {
            anyhow::bail!(
                "skill package file exceeds {max_file_bytes} byte limit: {}",
                path.display()
            );
        }
    }
    if bytes != file.expected_bytes {
        anyhow::bail!("package file changed while reading: {}", path.display());
    }
    Ok(bytes)
}

async fn copy_regular_file(
    source_root: &Path,
    destination_root: &Path,
    file: &PackageFile,
    max_file_bytes: u64,
) -> anyhow::Result<u64> {
    let source_path = source_root.join(&file.relative);
    let destination_path = destination_root.join(&file.relative);
    let metadata = tokio::fs::symlink_metadata(&source_path).await?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "package path changed while copying: {}",
            source_path.display()
        );
    }
    let mut source = tokio::fs::File::open(&source_path).await?;
    let mut destination = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&destination_path)
        .await?;
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    let mut bytes = 0_u64;
    loop {
        let count = source.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(u64::try_from(count)?)
            .context("package file size overflow")?;
        if bytes > max_file_bytes {
            anyhow::bail!(
                "skill package file exceeds {max_file_bytes} byte limit: {}",
                source_path.display()
            );
        }
        destination.write_all(&buffer[..count]).await?;
    }
    destination.flush().await?;
    if bytes != file.expected_bytes {
        anyhow::bail!(
            "package file changed while copying: {}",
            source_path.display()
        );
    }
    Ok(bytes)
}

fn checked_package_add(current: u64, bytes: u64, maximum: u64) -> anyhow::Result<u64> {
    let total = current
        .checked_add(bytes)
        .context("skill package byte count overflow")?;
    if total > maximum {
        anyhow::bail!("skill package exceeds {maximum} byte limit");
    }
    Ok(total)
}

async fn reject_existing(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => anyhow::bail!("skill store destination already exists: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn set_readonly(path: &Path, directory: bool) -> anyhow::Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if directory { 0o555 } else { 0o444 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    tokio::fs::set_permissions(path, permissions).await?;
    Ok(())
}

async fn set_writable(path: &Path, directory: bool) -> anyhow::Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if directory { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await?;
    Ok(())
}
