use crate::skill_source::canonical_relative_path;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectoryOwnership {
    Created,
    Existing,
}

pub(crate) async fn prepare_opened_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let ownership = prepare_directory_platform(&root, &relative)?;
        let directory = PreparedStoreDirectory::open(&root, &relative)?;
        Ok((directory, ownership))
    })
    .await
    .context("prepared directory ownership worker failed")?
}

pub(crate) async fn remove_opened_directory_if_empty(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<()> {
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || remove_opened_directory_if_empty_platform(&directory))
        .await
        .context("identity-bound empty directory cleanup worker failed")?
}

#[cfg(unix)]
fn prepare_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<DirectoryOwnership> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut parent = rustix::io::dup(root.descriptor())?;
    let components = relative.components().collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .context("prepared directory path is empty")?;
    for component in parents {
        parent = openat(
            &parent,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    match mkdirat(
        &parent,
        name.as_os_str(),
        Mode::from_raw_mode(RawMode::try_from(0o755_u32)?),
    ) {
        Ok(()) => Ok(DirectoryOwnership::Created),
        Err(rustix::io::Errno::EXIST) => Ok(DirectoryOwnership::Existing),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn remove_opened_directory_if_empty_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};
    directory.verify()?;
    let root = directory.root_identity();
    let mut parent = rustix::io::dup(root.descriptor())?;
    let components = directory.relative().components().collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .context("empty directory cleanup path is empty")?;
    for component in parents {
        parent = openat(
            &parent,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    directory.verify()?;
    unlinkat(&parent, name.as_os_str(), AtFlags::REMOVEDIR)?;
    Ok(())
}

#[cfg(windows)]
fn prepare_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<DirectoryOwnership> {
    let components = relative.components().collect::<Vec<_>>();
    let [component] = components.as_slice() else {
        anyhow::bail!("Windows prepared directory ownership requires one direct child");
    };
    let (_, _, _, created) = crate::skill_store_windows::create_or_open_directory_child(
        root.windows_descriptor(),
        root.windows_identity(),
        component.as_os_str(),
    )?;
    Ok(if created {
        DirectoryOwnership::Created
    } else {
        DirectoryOwnership::Existing
    })
}

#[cfg(windows)]
fn remove_opened_directory_if_empty_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<()> {
    directory.verify()?;
    crate::skill_store_windows::delete_opened_empty_directory(directory.windows_descriptor())
}

#[cfg(all(not(unix), not(windows)))]
fn prepare_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<DirectoryOwnership> {
    root.verify("prepared directory parent")?;
    match std::fs::create_dir(root.path().join(relative)) {
        Ok(()) => Ok(DirectoryOwnership::Created),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(DirectoryOwnership::Existing)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(not(unix), not(windows)))]
fn remove_opened_directory_if_empty_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<()> {
    directory.verify()?;
    std::fs::remove_dir(directory.path())?;
    Ok(())
}
