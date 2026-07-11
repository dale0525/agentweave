use crate::skill_source::{
    canonical_relative_path, portable_collision_key, register_portable_path,
};
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use anyhow::Context;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PackageLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
    pub max_entries: u64,
    pub max_files: u64,
    pub max_directories: u64,
    pub max_depth: u64,
    pub max_relative_path_bytes: u64,
}

#[derive(Debug)]
struct PackageEntries {
    root_mode: u32,
    directories: Vec<PackageDirectory>,
    files: Vec<PackageFile>,
}

#[derive(Debug)]
struct PackageDirectory {
    relative: PathBuf,
    mode: u32,
}

#[derive(Debug)]
struct PackageFile {
    relative: PathBuf,
    expected_bytes: u64,
    mode: u32,
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
    copy_collected_entries(source, destination, entries, limits, faults, copy_fault).await
}

pub(crate) async fn reserve_and_copy_package_tree(
    source: &Path,
    destination: &Path,
    limits: PackageLimits,
    faults: &StoreFaults,
    copy_fault: StoreFaultPoint,
) -> anyhow::Result<()> {
    let entries = collect_entries(source, limits).await?;
    match tokio::fs::create_dir(destination).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!(
                "skill store destination already exists: {}",
                destination.display()
            )
        }
        Err(error) => return Err(error.into()),
    }
    let copy =
        copy_collected_entries(source, destination, entries, limits, faults, copy_fault).await;
    match copy {
        Ok(()) => Ok(()),
        Err(error) => match tokio::fs::remove_dir_all(destination).await {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!(
                "reserved destination cleanup failed for {}: {cleanup}",
                destination.display()
            ))),
        },
    }
}

async fn copy_collected_entries(
    source: &Path,
    destination: &Path,
    entries: PackageEntries,
    limits: PackageLimits,
    faults: &StoreFaults,
    copy_fault: StoreFaultPoint,
) -> anyhow::Result<()> {
    for directory in &entries.directories {
        tokio::fs::create_dir(destination.join(&directory.relative))
            .await
            .with_context(|| {
                format!(
                    "failed to create package directory {}",
                    destination.join(&directory.relative).display()
                )
            })?;
    }

    let mut package_bytes = 0_u64;
    for file in entries.files {
        faults.check(copy_fault)?;
        faults.checkpoint(StoreFaultPoint::CopyBeforeFileOpen).await;
        let bytes = copy_regular_file(source, destination, &file, limits.max_file_bytes).await?;
        package_bytes = checked_package_add(package_bytes, bytes, limits.max_package_bytes)?;
    }
    let mut directories = entries.directories;
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.relative.components().count()));
    for directory in directories {
        set_safe_mode(&destination.join(directory.relative), directory.mode).await?;
    }
    set_safe_mode(destination, entries.root_mode).await?;
    Ok(())
}

pub(crate) async fn ensure_safe_write_parent(
    root: &Path,
    relative: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
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
    let mut created = Vec::new();
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
                    created.push(current.clone());
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
    Ok(created)
}

pub(crate) async fn ensure_directory_contained(
    store_root: &Path,
    directory: &Path,
    label: &str,
) -> anyhow::Result<()> {
    let root_metadata = tokio::fs::symlink_metadata(store_root)
        .await
        .with_context(|| {
            format!(
                "failed to inspect {label} store root {}",
                store_root.display()
            )
        })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        anyhow::bail!(
            "{label} store root must be a real directory: {}",
            store_root.display()
        );
    }
    let relative = directory.strip_prefix(store_root).with_context(|| {
        format!(
            "{label} directory is not lexically beneath store root: {}",
            directory.display()
        )
    })?;
    let mut current = store_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = tokio::fs::symlink_metadata(&current)
            .await
            .with_context(|| format!("failed to inspect {label} path {}", current.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            anyhow::bail!(
                "{label} path must contain only real directories: {}",
                current.display()
            );
        }
    }
    let directory_metadata = tokio::fs::symlink_metadata(directory)
        .await
        .with_context(|| {
            format!(
                "failed to inspect {label} directory {}",
                directory.display()
            )
        })?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        anyhow::bail!(
            "{label} directory must be a real directory: {}",
            directory.display()
        );
    }
    let canonical_root = tokio::fs::canonicalize(store_root).await?;
    let canonical_directory = tokio::fs::canonicalize(directory).await?;
    if !canonical_directory.starts_with(&canonical_root) {
        anyhow::bail!(
            "{label} directory escapes store root: {}",
            directory.display()
        );
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct StoredFileContents {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

pub(crate) async fn read_optional_regular_file(
    root: &Path,
    relative: &Path,
    max_file_bytes: u64,
) -> anyhow::Result<Option<StoredFileContents>> {
    let path = root.join(relative);
    match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!(
                "staging destination must be a regular file: {}",
                path.display()
            )
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let (mut file, opened_bytes, mode) = open_regular_file_nofollow(root, relative).await?;
    if opened_bytes > max_file_bytes {
        anyhow::bail!("staging file exceeds {max_file_bytes} byte limit");
    }
    let capacity = usize::try_from(opened_bytes).context("staging file is too large to buffer")?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes).await?;
    if u64::try_from(bytes.len())? != opened_bytes {
        anyhow::bail!("staging file changed while reading: {}", path.display());
    }
    Ok(Some(StoredFileContents { bytes, mode }))
}

pub(crate) async fn atomic_replace_file(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
    faults: &StoreFaults,
) -> anyhow::Result<()> {
    faults
        .checkpoint(StoreFaultPoint::WriteBeforeTempOpen)
        .await;
    atomic_replace_file_platform(root, relative, bytes, mode).await
}

pub(crate) async fn remove_created_directories(created: &[PathBuf]) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for path in created.iter().rev() {
        match tokio::fs::remove_dir(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to remove created staging directories: {}",
            errors.join("; ")
        )
    }
}

#[cfg(unix)]
pub(crate) async fn remove_regular_file_nofollow(
    root: &Path,
    relative: &Path,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, unlinkat};

    let (parent, name) = open_parent_nofollow(root, relative)?;
    match unlinkat(&parent, name, AtFlags::empty()) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
pub(crate) async fn remove_regular_file_nofollow(
    root: &Path,
    relative: &Path,
) -> anyhow::Result<()> {
    let path = root.join(relative);
    let canonical_root = tokio::fs::canonicalize(root).await?;
    let canonical_parent =
        tokio::fs::canonicalize(path.parent().context("staging file has no parent")?).await?;
    if !canonical_parent.starts_with(&canonical_root) {
        anyhow::bail!(
            "staging file parent escapes revision root: {}",
            path.display()
        );
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn make_tree_readonly(root: &Path, limits: PackageLimits) -> anyhow::Result<()> {
    let entries = collect_entries(root, limits).await?;
    for file in entries.files {
        set_readonly(&root.join(file.relative)).await?;
    }
    let mut directories = entries.directories;
    directories.sort_by_key(|directory| std::cmp::Reverse(directory.relative.components().count()));
    for directory in directories {
        set_readonly(&root.join(directory.relative)).await?;
    }
    set_readonly(root).await
}

pub(crate) async fn make_tree_writable(root: &Path, limits: PackageLimits) -> anyhow::Result<()> {
    let entries = collect_entries(root, limits).await?;
    set_writable(root, true).await?;
    for directory in &entries.directories {
        set_writable(&root.join(&directory.relative), true).await?;
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
    let mut entry_count = 0_u64;
    let mut file_count = 0_u64;
    let mut directory_count = 0_u64;
    while let Some(relative_directory) = stack.pop() {
        let directory = root.join(&relative_directory);
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to read package directory {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let relative = relative_directory.join(entry.file_name());
            canonical_relative_path(&relative)?;
            let relative_bytes = u64::try_from(relative.as_os_str().as_encoded_bytes().len())?;
            if relative_bytes > limits.max_relative_path_bytes {
                anyhow::bail!(
                    "skill package relative path exceeds {} byte limit: {}",
                    limits.max_relative_path_bytes,
                    relative.display()
                );
            }
            let depth = u64::try_from(relative.components().count())?;
            if depth > limits.max_depth {
                anyhow::bail!(
                    "skill package path depth exceeds {} component limit: {}",
                    limits.max_depth,
                    relative.display()
                );
            }
            entry_count = checked_count_add(entry_count, limits.max_entries, "entry")?;
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
                directory_count =
                    checked_count_add(directory_count, limits.max_directories, "directory")?;
                directories.push(PackageDirectory {
                    relative: relative.clone(),
                    mode: safe_mode(&metadata, true),
                });
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
            file_count = checked_count_add(file_count, limits.max_files, "file")?;
            files.push(PackageFile {
                relative,
                expected_bytes: metadata.len(),
                mode: safe_mode(&metadata, false),
            });
        }
    }
    directories.sort_by(|left, right| left.relative.cmp(&right.relative));
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(PackageEntries {
        root_mode: safe_mode(&metadata, true),
        directories,
        files,
    })
}

async fn read_file_size(
    root: &Path,
    file: &PackageFile,
    max_file_bytes: u64,
) -> anyhow::Result<u64> {
    let path = root.join(&file.relative);
    let (mut source, opened_bytes, _) = open_regular_file_nofollow(root, &file.relative).await?;
    if opened_bytes != file.expected_bytes {
        anyhow::bail!("package file changed while reading: {}", path.display());
    }
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
    let (mut source, opened_bytes, _) =
        open_regular_file_nofollow(source_root, &file.relative).await?;
    if opened_bytes != file.expected_bytes {
        anyhow::bail!(
            "package file changed while copying: {}",
            source_path.display()
        );
    }
    let mut destination =
        create_regular_file_nofollow(destination_root, &file.relative, file.mode).await?;
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
    set_safe_mode(&destination_path, file.mode).await?;
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

fn checked_count_add(current: u64, maximum: u64, kind: &str) -> anyhow::Result<u64> {
    let total = current
        .checked_add(1)
        .with_context(|| format!("skill package {kind} count overflow"))?;
    if total > maximum {
        anyhow::bail!("skill package {kind} count exceeds {maximum} limit");
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

async fn set_readonly(path: &Path) -> anyhow::Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() & !0o222);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(true);
    tokio::fs::set_permissions(path, permissions).await?;
    Ok(())
}

fn safe_mode(metadata: &std::fs::Metadata, _directory: bool) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        if _directory { 0o755 } else { 0o644 }
    }
}

async fn set_safe_mode(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777)).await?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

#[cfg(unix)]
async fn open_regular_file_nofollow(
    root: &Path,
    relative: &Path,
) -> anyhow::Result<(tokio::fs::File, u64, u32)> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, openat};
    use std::fs::File;

    let (parent, name) = open_parent_nofollow(root, relative)?;
    let descriptor = openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open package file without following symlinks: {}",
            root.join(relative).display()
        )
    })?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!(
            "package path is not a regular file after no-follow open: {}",
            root.join(relative).display()
        );
    }
    let bytes = u64::try_from(stat.st_size).context("package file has negative size")?;
    let mode = u32::from(stat.st_mode) & 0o777;
    Ok((
        tokio::fs::File::from_std(File::from(descriptor)),
        bytes,
        mode,
    ))
}

#[cfg(unix)]
async fn atomic_replace_file_platform(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode, fstat, openat, renameat, unlinkat};
    use std::fs::File;

    let (parent, destination_name) = open_parent_nofollow(root, relative)?;
    let temporary_name = format!(".skill-write-{}.tmp", uuid::Uuid::new_v4());
    let descriptor = openat(
        &parent,
        temporary_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(mode & 0o777)?),
    )
    .with_context(|| {
        format!(
            "failed to create staging temporary file without following symlinks: {}",
            root.join(relative).display()
        )
    })?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!("staging temporary path is not a regular file");
    }
    let mut file = tokio::fs::File::from_std(File::from(descriptor));
    let result = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        renameat(&parent, temporary_name.as_str(), &parent, destination_name).with_context(
            || {
                format!(
                    "failed to atomically replace staging file {}",
                    root.join(relative).display()
                )
            },
        )?;
        set_safe_mode(&root.join(relative), mode).await?;
        let _ = open_regular_file_nofollow(root, relative).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = unlinkat(&parent, temporary_name.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(not(unix))]
async fn atomic_replace_file_platform(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    _mode: u32,
) -> anyhow::Result<()> {
    let destination = root.join(relative);
    let parent = destination.parent().context("staging file has no parent")?;
    let canonical_root = tokio::fs::canonicalize(root).await?;
    let canonical_parent = tokio::fs::canonicalize(parent).await?;
    if !canonical_parent.starts_with(&canonical_root) {
        anyhow::bail!(
            "staging file parent escapes revision root: {}",
            parent.display()
        );
    }
    let temporary = parent.join(format!(".skill-write-{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        if !file.metadata().await?.is_file() {
            anyhow::bail!("staging temporary path is not a regular file");
        }
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        let canonical_parent = tokio::fs::canonicalize(parent).await?;
        if !canonical_parent.starts_with(&canonical_root) {
            anyhow::bail!("staging file parent escaped revision root before replace");
        }
        tokio::fs::rename(&temporary, &destination).await?;
        let _ = open_regular_file_nofollow(root, relative).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

#[cfg(not(unix))]
async fn open_regular_file_nofollow(
    root: &Path,
    relative: &Path,
) -> anyhow::Result<(tokio::fs::File, u64, u32)> {
    let path = root.join(relative);
    let root = tokio::fs::canonicalize(root).await?;
    let parent =
        tokio::fs::canonicalize(path.parent().context("package file has no parent")?).await?;
    if !parent.starts_with(&root) {
        anyhow::bail!(
            "package file parent escapes package root: {}",
            path.display()
        );
    }
    let file = tokio::fs::File::open(&path).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        anyhow::bail!("package path is not a regular file: {}", path.display());
    }
    let canonical_path = tokio::fs::canonicalize(&path).await?;
    if !canonical_path.starts_with(&root) {
        anyhow::bail!(
            "opened package file escapes package root: {}",
            path.display()
        );
    }
    Ok((file, metadata.len(), safe_mode(&metadata, false)))
}

#[cfg(unix)]
async fn create_regular_file_nofollow(
    root: &Path,
    relative: &Path,
    mode: u32,
) -> anyhow::Result<tokio::fs::File> {
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, openat};
    use std::fs::File;

    let (parent, name) = open_parent_nofollow(root, relative)?;
    let descriptor = openat(
        &parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(mode & 0o777)?),
    )
    .with_context(|| {
        format!(
            "failed to create package file without following symlinks: {}",
            root.join(relative).display()
        )
    })?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!(
            "created package path is not a regular file: {}",
            root.join(relative).display()
        );
    }
    Ok(tokio::fs::File::from_std(File::from(descriptor)))
}

#[cfg(not(unix))]
async fn create_regular_file_nofollow(
    root: &Path,
    relative: &Path,
    _mode: u32,
) -> anyhow::Result<tokio::fs::File> {
    let path = root.join(relative);
    let root = tokio::fs::canonicalize(root).await?;
    let parent =
        tokio::fs::canonicalize(path.parent().context("package file has no parent")?).await?;
    if !parent.starts_with(&root) {
        anyhow::bail!(
            "package destination escapes package root: {}",
            path.display()
        );
    }
    let file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await?;
    if !file.metadata().await?.is_file() {
        anyhow::bail!(
            "package destination is not a regular file: {}",
            path.display()
        );
    }
    let canonical_path = tokio::fs::canonicalize(&path).await?;
    if !canonical_path.starts_with(&root) {
        anyhow::bail!(
            "created package file escapes package root: {}",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(unix)]
fn open_parent_nofollow<'a>(
    root: &Path,
    relative: &'a Path,
) -> anyhow::Result<(std::os::fd::OwnedFd, &'a std::ffi::OsStr)> {
    use rustix::fs::{Mode, OFlags, open, openat};

    canonical_relative_path(relative)?;
    let mut directory = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open package root without following symlinks: {}",
            root.display()
        )
    })?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        directory = openat(
            &directory,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .with_context(|| {
            format!(
                "failed to open package parent without following symlinks: {}",
                root.join(parent).display()
            )
        })?;
    }
    let name = relative
        .file_name()
        .context("package relative file path has no name")?;
    Ok((directory, name))
}

async fn set_writable(path: &Path, directory: bool) -> anyhow::Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let owner_access = if directory { 0o700 } else { 0o600 };
        permissions.set_mode(permissions.mode() | owner_access);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await?;
    Ok(())
}
