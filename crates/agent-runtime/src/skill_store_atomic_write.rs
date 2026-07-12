use crate::skill_source::canonical_relative_path;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use crate::skill_store_fs_types::{AtomicReplaceCommitState, AtomicReplaceFailure};
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;
use std::path::Path;
use tokio::io::{AsyncWriteExt, BufWriter};

pub(crate) async fn atomic_replace_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
    faults: &StoreFaults,
) -> Result<(), AtomicReplaceFailure> {
    atomic_replace_file_with_destination_sharing(root, relative, bytes, mode, faults, false).await
}

pub(crate) async fn atomic_replace_replaceable_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
    faults: &StoreFaults,
) -> Result<(), AtomicReplaceFailure> {
    atomic_replace_file_with_destination_sharing(root, relative, bytes, mode, faults, true).await
}

async fn atomic_replace_file_with_destination_sharing(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
    faults: &StoreFaults,
    replaceable_destination: bool,
) -> Result<(), AtomicReplaceFailure> {
    faults
        .checkpoint(StoreFaultPoint::WriteBeforeTempOpen)
        .await;
    root.verify().map_err(not_committed)?;
    atomic_replace_file_platform(root, relative, bytes, mode, faults, replaceable_destination).await
}

fn not_committed(error: anyhow::Error) -> AtomicReplaceFailure {
    AtomicReplaceFailure {
        state: AtomicReplaceCommitState::NotCommitted,
        temp_path: None,
        error,
    }
}

fn failure(
    error: anyhow::Error,
    state: AtomicReplaceCommitState,
    temp_path: Option<std::path::PathBuf>,
) -> AtomicReplaceFailure {
    AtomicReplaceFailure {
        state,
        temp_path,
        error,
    }
}

#[cfg(unix)]
async fn atomic_replace_file_platform(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
    faults: &StoreFaults,
    _replaceable_destination: bool,
) -> Result<(), AtomicReplaceFailure> {
    use rustix::fs::{
        AtFlags, FileType, Mode, OFlags, RawMode, fchmod, fstat, openat, renameat, unlinkat,
    };
    use std::fs::File;

    let (parent, destination_name) = open_parent(root, relative).map_err(not_committed)?;
    let temporary_name = format!(".skill-write-{}.tmp", uuid::Uuid::new_v4());
    let temporary_path = root
        .path()
        .join(relative.parent().unwrap_or_else(|| Path::new("")))
        .join(&temporary_name);
    let descriptor = openat(
        &parent,
        temporary_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(mode & 0o777).map_err(|error| {
            failure(error.into(), AtomicReplaceCommitState::NotCommitted, None)
        })?),
    )
    .with_context(|| {
        format!(
            "failed to create staging temporary file without following symlinks: {}",
            root.path().join(relative).display()
        )
    })
    .map_err(not_committed)?;
    let stat = fstat(&descriptor).map_err(|error| not_committed(error.into()))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(failure(
            anyhow::anyhow!("staging temporary path is not a regular file"),
            AtomicReplaceCommitState::NotCommitted,
            Some(temporary_path),
        ));
    }
    let mut file = BufWriter::new(tokio::fs::File::from_std(File::from(descriptor)));
    let mut committed = false;
    let result = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        faults.check(StoreFaultPoint::WriteBeforeRename)?;
        renameat(&parent, temporary_name.as_str(), &parent, destination_name).with_context(
            || {
                format!(
                    "failed to atomically replace staging file {}",
                    root.path().join(relative).display()
                )
            },
        )?;
        committed = true;
        faults.check(StoreFaultPoint::WriteAfterRenameMode)?;
        let destination = openat(
            &parent,
            destination_name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let stat = fstat(&destination)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            anyhow::bail!("staging destination is not a regular file after replace");
        }
        fchmod(
            &destination,
            Mode::from_raw_mode(RawMode::try_from(mode & 0o777)?),
        )?;
        faults.check(StoreFaultPoint::WriteAfterRenameRevalidate)?;
        root.verify()?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match result {
        Ok(()) => Ok(()),
        Err(error) if committed => Err(failure(error, AtomicReplaceCommitState::Committed, None)),
        Err(error) => {
            let cleanup = match faults.check(StoreFaultPoint::WriteTempCleanup) {
                Ok(()) => unlinkat(&parent, temporary_name.as_str(), AtFlags::empty())
                    .map_err(anyhow::Error::from),
                Err(cleanup) => Err(cleanup),
            };
            finish_uncommitted(error, cleanup, temporary_path)
        }
    }
}

#[cfg(unix)]
fn open_parent<'a>(
    root: &PreparedStoreDirectory,
    relative: &'a Path,
) -> anyhow::Result<(std::os::fd::OwnedFd, &'a std::ffi::OsStr)> {
    use rustix::fs::{Mode, OFlags, openat};
    canonical_relative_path(relative)?;
    let mut directory = rustix::io::dup(root.descriptor())?;
    for component in relative
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
    {
        directory = openat(
            &directory,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    let name = relative
        .file_name()
        .context("package relative file path has no name")?;
    Ok((directory, name))
}

#[cfg(windows)]
async fn atomic_replace_file_platform(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    _mode: u32,
    faults: &StoreFaults,
    replaceable_destination: bool,
) -> Result<(), AtomicReplaceFailure> {
    canonical_relative_path(relative).map_err(not_committed)?;
    let (parent, destination_name) =
        crate::skill_store_windows::open_stable_parent(root.windows_descriptor(), relative)
            .map_err(not_committed)?;
    let temporary_name =
        std::ffi::OsString::from(format!(".skill-write-{}.tmp", uuid::Uuid::new_v4()));
    let temporary = parent.child_path(&temporary_name);
    let mut committed = false;
    let result = async {
        let mut file = tokio::fs::File::from_std(parent.create_new_regular(&temporary_name)?);
        if !file.metadata().await?.is_file() {
            anyhow::bail!("staging temporary path is not a regular file");
        }
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        root.verify()?;
        faults.check(StoreFaultPoint::WriteBeforeRename)?;
        parent.atomic_replace(&temporary_name, &destination_name, replaceable_destination)?;
        committed = true;
        faults.check(StoreFaultPoint::WriteAfterRenameMode)?;
        faults.check(StoreFaultPoint::WriteAfterRenameRevalidate)?;
        root.verify()?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match result {
        Ok(()) => Ok(()),
        Err(error) if committed => Err(failure(error, AtomicReplaceCommitState::Committed, None)),
        Err(error) => {
            let cleanup = match faults.check(StoreFaultPoint::WriteTempCleanup) {
                Ok(()) => parent.remove_regular(&temporary_name),
                Err(cleanup) => Err(cleanup),
            };
            finish_uncommitted(error, cleanup, temporary)
        }
    }
}

#[cfg(all(not(unix), not(windows)))]
async fn atomic_replace_file_platform(
    root: &PreparedStoreDirectory,
    relative: &Path,
    bytes: &[u8],
    _mode: u32,
    faults: &StoreFaults,
    _replaceable_destination: bool,
) -> Result<(), AtomicReplaceFailure> {
    canonical_relative_path(relative).map_err(not_committed)?;
    let destination = root.path().join(relative);
    let parent = destination
        .parent()
        .context("staging file has no parent")
        .map_err(not_committed)?;
    let temporary = parent.join(format!(".skill-write-{}.tmp", uuid::Uuid::new_v4()));
    let mut committed = false;
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        file.write_all(bytes).await?;
        file.flush().await?;
        drop(file);
        root.verify()?;
        faults.check(StoreFaultPoint::WriteBeforeRename)?;
        tokio::fs::rename(&temporary, &destination).await?;
        committed = true;
        faults.check(StoreFaultPoint::WriteAfterRenameMode)?;
        faults.check(StoreFaultPoint::WriteAfterRenameRevalidate)?;
        root.verify()?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    finish_platform_result(result, committed, faults, temporary).await
}

#[cfg(all(not(unix), not(windows)))]
async fn finish_platform_result(
    result: anyhow::Result<()>,
    committed: bool,
    faults: &StoreFaults,
    temporary: std::path::PathBuf,
) -> Result<(), AtomicReplaceFailure> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if committed => Err(failure(error, AtomicReplaceCommitState::Committed, None)),
        Err(error) => {
            let cleanup = match faults.check(StoreFaultPoint::WriteTempCleanup) {
                Ok(()) => tokio::fs::remove_file(&temporary)
                    .await
                    .map_err(anyhow::Error::from),
                Err(cleanup) => Err(cleanup),
            };
            finish_uncommitted(error, cleanup, temporary)
        }
    }
}

fn finish_uncommitted(
    error: anyhow::Error,
    cleanup: anyhow::Result<()>,
    temporary: std::path::PathBuf,
) -> Result<(), AtomicReplaceFailure> {
    match cleanup {
        Ok(()) => Err(failure(error, AtomicReplaceCommitState::NotCommitted, None)),
        Err(cleanup) if cleanup_is_not_found(&cleanup) => {
            Err(failure(error, AtomicReplaceCommitState::NotCommitted, None))
        }
        Err(cleanup) => Err(failure(
            error.context(format!("temporary cleanup failed: {cleanup:#}")),
            AtomicReplaceCommitState::NotCommitted,
            Some(temporary),
        )),
    }
}

fn cleanup_is_not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        || {
            #[cfg(unix)]
            {
                error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
}
