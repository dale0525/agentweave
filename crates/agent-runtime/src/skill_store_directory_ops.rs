use crate::skill_source::canonical_relative_path;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct OwnedDirectoryBootstrap {
    directory: PreparedStoreDirectory,
}

pub(crate) enum DirectoryOwnership {
    Created(OwnedDirectoryBootstrap),
    Existing,
}

pub(crate) async fn prepare_opened_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || prepare_directory_platform(&root, &relative))
        .await
        .context("prepared directory ownership worker failed")?
}

pub(crate) async fn remove_owned_directory_if_empty(
    owned: &OwnedDirectoryBootstrap,
) -> anyhow::Result<()> {
    let directory = owned.directory.clone();
    tokio::task::spawn_blocking(move || remove_opened_directory_if_empty_platform(&directory))
        .await
        .context("identity-bound empty directory cleanup worker failed")?
}

#[cfg(unix)]
fn prepare_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    use rustix::fs::{Mode, OFlags, openat};

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

    #[cfg(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))]
    {
        use rustix::fs::{AtFlags, RawMode, RenameFlags, mkdirat, renameat_with, unlinkat};

        let private_name = format!(".skill-bootstrap-{}", uuid::Uuid::new_v4());
        mkdirat(
            &parent,
            private_name.as_str(),
            Mode::from_raw_mode(RawMode::try_from(0o755_u32)?),
        )?;
        let opened = match openat(
            &parent,
            private_name.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(opened) => opened,
            Err(error) => {
                let _ = unlinkat(&parent, private_name.as_str(), AtFlags::REMOVEDIR);
                return Err(error.into());
            }
        };
        match renameat_with(
            &parent,
            private_name.as_str(),
            &parent,
            name.as_os_str(),
            RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {
                let directory = PreparedStoreDirectory::from_opened(
                    root,
                    relative,
                    std::fs::File::from(opened),
                )?;
                let owned = OwnedDirectoryBootstrap {
                    directory: directory.clone(),
                };
                Ok((directory, DirectoryOwnership::Created(owned)))
            }
            Err(rustix::io::Errno::EXIST) => {
                remove_exact_private_directory(&parent, private_name.as_str(), &opened)?;
                Ok((
                    PreparedStoreDirectory::open(root, relative)?,
                    DirectoryOwnership::Existing,
                ))
            }
            Err(error) => {
                let cleanup =
                    remove_exact_private_directory(&parent, private_name.as_str(), &opened);
                match cleanup {
                    Ok(()) => Err(error.into()),
                    Err(cleanup) => Err(anyhow::Error::from(error).context(format!(
                        "private directory publication failed and cleanup retained evidence: {cleanup:#}"
                    ))),
                }
            }
        }
    }

    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    )))]
    {
        let _ = (parent, name);
        anyhow::bail!(
            "atomic no-replace directory publication is unsupported on this Unix platform"
        )
    }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn remove_exact_private_directory(
    parent: &impl std::os::fd::AsFd,
    name: &str,
    opened: &impl std::os::fd::AsFd,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, fstat, statat, unlinkat};

    let expected = fstat(opened)?;
    let current = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    anyhow::ensure!(
        FileType::from_raw_mode(current.st_mode) == FileType::Directory
            && current.st_dev == expected.st_dev
            && current.st_ino == expected.st_ino,
        "private directory ownership changed before cleanup"
    );
    unlinkat(parent, name, AtFlags::REMOVEDIR)?;
    Ok(())
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
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    let components = relative.components().collect::<Vec<_>>();
    let [component] = components.as_slice() else {
        anyhow::bail!("Windows prepared directory ownership requires one direct child");
    };
    let (descriptor, _, _, created) = crate::skill_store_windows::create_or_open_directory_child(
        root.windows_descriptor(),
        root.windows_identity(),
        component.as_os_str(),
    )?;
    let directory = PreparedStoreDirectory::from_opened(root, relative, descriptor)?;
    let ownership = if created {
        DirectoryOwnership::Created(OwnedDirectoryBootstrap {
            directory: directory.clone(),
        })
    } else {
        DirectoryOwnership::Existing
    };
    Ok((directory, ownership))
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
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    root.verify("prepared directory parent")?;
    let created = match std::fs::create_dir(root.path().join(relative)) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };
    let directory = PreparedStoreDirectory::open(root, relative)?;
    let ownership = if created {
        DirectoryOwnership::Created(OwnedDirectoryBootstrap {
            directory: directory.clone(),
        })
    } else {
        DirectoryOwnership::Existing
    };
    Ok((directory, ownership))
}

#[cfg(all(not(unix), not(windows)))]
fn remove_opened_directory_if_empty_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<()> {
    directory.verify()?;
    std::fs::remove_dir(directory.path())?;
    Ok(())
}
