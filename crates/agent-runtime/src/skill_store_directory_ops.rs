use crate::skill_source::canonical_relative_path;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
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
    faults: &StoreFaults,
    before_move: StoreFaultPoint,
    before_delete: StoreFaultPoint,
) -> anyhow::Result<()> {
    let directory = owned.directory.clone();
    let target = tokio::task::spawn_blocking(move || prepare_owned_cleanup_platform(&directory))
        .await
        .context("identity-bound empty directory cleanup preparation worker failed")??;
    faults.checkpoint(before_move).await;
    let quarantine = tokio::task::spawn_blocking(move || quarantine_owned_cleanup_platform(target))
        .await
        .context("identity-bound empty directory quarantine worker failed")??;
    faults.checkpoint(before_delete).await;
    tokio::task::spawn_blocking(move || remove_quarantined_directory_if_empty_platform(quarantine))
        .await
        .context("identity-bound empty directory deletion worker failed")?
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
    let display_parent = root
        .path()
        .join(relative)
        .parent()
        .context("prepared directory path has no parent")?
        .to_path_buf();
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
                remove_exact_private_directory(
                    &parent,
                    private_name.as_ref(),
                    &opened,
                    &display_parent,
                )?;
                Ok((
                    PreparedStoreDirectory::open(root, relative)?,
                    DirectoryOwnership::Existing,
                ))
            }
            Err(error) => {
                let cleanup = remove_exact_private_directory(
                    &parent,
                    private_name.as_ref(),
                    &opened,
                    &display_parent,
                );
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
    name: &std::ffi::OsStr,
    opened: &impl std::os::fd::AsFd,
    display_parent: &Path,
) -> anyhow::Result<()> {
    let quarantine_path = display_parent.join(format!(
        ".skill-cleanup-quarantine-{}",
        uuid::Uuid::new_v4()
    ));
    let quarantine = quarantine_named_directory(parent, name, opened, &quarantine_path)?;
    remove_quarantined_directory(quarantine)
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn prepare_owned_cleanup_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<OwnedCleanupTarget> {
    use rustix::fs::{Mode, OFlags, openat};

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
    Ok(OwnedCleanupTarget {
        parent,
        name: name.as_os_str().to_os_string(),
        expected: directory.descriptor().try_clone()?,
        quarantine_path: directory.path().with_file_name(format!(
            ".skill-cleanup-quarantine-{}",
            uuid::Uuid::new_v4()
        )),
    })
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
struct OwnedCleanupTarget {
    parent: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
    expected: std::fs::File,
    quarantine_path: std::path::PathBuf,
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn quarantine_owned_cleanup_platform(
    target: OwnedCleanupTarget,
) -> anyhow::Result<QuarantinedDirectory> {
    quarantine_named_directory(
        &target.parent,
        &target.name,
        &target.expected,
        &target.quarantine_path,
    )
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
#[derive(Debug)]
struct QuarantinedDirectory {
    parent: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
    opened: std::os::fd::OwnedFd,
    path: std::path::PathBuf,
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn quarantine_named_directory(
    parent: &impl std::os::fd::AsFd,
    name: &std::ffi::OsStr,
    expected: &impl std::os::fd::AsFd,
    quarantine_path: &Path,
) -> anyhow::Result<QuarantinedDirectory> {
    use rustix::fs::{FileType, Mode, OFlags, RenameFlags, fstat, openat, renameat_with};

    let parent = rustix::io::dup(parent)?;
    let quarantine_name = quarantine_path
        .file_name()
        .context("cleanup quarantine has no file name")?
        .to_os_string();
    renameat_with(
        &parent,
        name,
        &parent,
        &quarantine_name,
        RenameFlags::NOREPLACE,
    )
    .with_context(|| {
        format!(
            "failed to move owned directory into cleanup quarantine {}",
            quarantine_path.display()
        )
    })?;
    let opened = openat(
        &parent,
        &quarantine_name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open cleanup quarantine; retained evidence at {}",
            quarantine_path.display()
        )
    })?;
    let expected = fstat(expected)?;
    let moved = fstat(&opened)?;
    anyhow::ensure!(
        FileType::from_raw_mode(moved.st_mode) == FileType::Directory
            && moved.st_dev == expected.st_dev
            && moved.st_ino == expected.st_ino,
        "cleanup quarantine contains a foreign directory; retained evidence at {}",
        quarantine_path.display()
    );
    Ok(QuarantinedDirectory {
        parent,
        name: quarantine_name,
        opened,
        path: quarantine_path.to_path_buf(),
    })
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn remove_quarantined_directory(quarantine: QuarantinedDirectory) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, fstat, statat, unlinkat};

    let expected = fstat(&quarantine.opened)?;
    let current = statat(
        &quarantine.parent,
        &quarantine.name,
        AtFlags::SYMLINK_NOFOLLOW,
    )?;
    anyhow::ensure!(
        FileType::from_raw_mode(current.st_mode) == FileType::Directory
            && current.st_dev == expected.st_dev
            && current.st_ino == expected.st_ino,
        "cleanup quarantine identity changed before deletion; retained evidence at {}",
        quarantine.path.display()
    );
    unlinkat(&quarantine.parent, &quarantine.name, AtFlags::REMOVEDIR).with_context(|| {
        format!(
            "cleanup quarantine could not be removed; retained evidence at {}",
            quarantine.path.display()
        )
    })?;
    Ok(())
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))
))]
struct QuarantinedDirectory;

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))
))]
struct OwnedCleanupTarget;

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))
))]
fn prepare_owned_cleanup_platform(
    _directory: &PreparedStoreDirectory,
) -> anyhow::Result<OwnedCleanupTarget> {
    anyhow::bail!("atomic cleanup quarantine is unsupported on this Unix platform")
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))
))]
fn quarantine_named_directory(
    _parent: &impl std::os::fd::AsFd,
    _name: &std::ffi::OsStr,
    _expected: &impl std::os::fd::AsFd,
    _quarantine_path: &Path,
) -> anyhow::Result<QuarantinedDirectory> {
    anyhow::bail!("atomic cleanup quarantine is unsupported on this Unix platform")
}

#[cfg(all(
    unix,
    not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))
))]
fn quarantine_owned_cleanup_platform(
    _target: OwnedCleanupTarget,
) -> anyhow::Result<QuarantinedDirectory> {
    anyhow::bail!("atomic cleanup quarantine is unsupported on this Unix platform")
}

#[cfg(unix)]
fn remove_quarantined_directory_if_empty_platform(
    quarantine: QuarantinedDirectory,
) -> anyhow::Result<()> {
    #[cfg(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    ))]
    return remove_quarantined_directory(quarantine);
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    )))]
    anyhow::bail!("atomic cleanup quarantine is unsupported on this Unix platform")
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
fn prepare_owned_cleanup_platform(
    directory: &PreparedStoreDirectory,
) -> anyhow::Result<PreparedStoreDirectory> {
    directory.verify()?;
    Ok(directory.clone())
}

#[cfg(windows)]
fn quarantine_owned_cleanup_platform(
    directory: PreparedStoreDirectory,
) -> anyhow::Result<PreparedStoreDirectory> {
    Ok(directory)
}

#[cfg(windows)]
fn remove_quarantined_directory_if_empty_platform(
    directory: PreparedStoreDirectory,
) -> anyhow::Result<()> {
    directory.verify()?;
    crate::skill_store_windows::delete_opened_empty_directory(directory.windows_descriptor())
}

#[cfg(all(not(unix), not(windows)))]
fn prepare_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<(PreparedStoreDirectory, DirectoryOwnership)> {
    let _ = (root, relative);
    unsupported_bundle_directory_primitive()
}

#[cfg(all(not(unix), not(windows)))]
fn prepare_owned_cleanup_platform(_directory: &PreparedStoreDirectory) -> anyhow::Result<()> {
    unsupported_bundle_directory_primitive()
}

#[cfg(all(not(unix), not(windows)))]
fn quarantine_owned_cleanup_platform(_target: ()) -> anyhow::Result<()> {
    unsupported_bundle_directory_primitive()
}

#[cfg(all(not(unix), not(windows)))]
fn remove_quarantined_directory_if_empty_platform(_quarantine: ()) -> anyhow::Result<()> {
    unsupported_bundle_directory_primitive()
}

#[cfg(any(test, all(not(unix), not(windows))))]
fn unsupported_bundle_directory_primitive<T>() -> anyhow::Result<T> {
    anyhow::bail!("bundle directory publication is unsupported on this platform")
}

#[cfg(test)]
#[test]
fn unsupported_bundle_directory_contract_fails_closed() {
    let error = unsupported_bundle_directory_primitive::<()>().unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("unsupported") && message.contains("platform"));
}

#[cfg(all(
    test,
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "redox"
    )
))]
mod tests {
    use super::*;

    #[test]
    fn private_loser_cleanup_quarantines_foreign_replacement_before_move() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join(".skill-bootstrap-owned");
        let displaced = temp.path().join("owned-evidence");
        let quarantine = temp.path().join(".skill-cleanup-quarantine-test");
        std::fs::create_dir(&private).unwrap();
        let opened = std::fs::File::open(&private).unwrap();
        let parent = std::fs::File::open(temp.path()).unwrap();
        std::fs::rename(&private, &displaced).unwrap();
        std::fs::create_dir(&private).unwrap();

        let error =
            quarantine_named_directory(&parent, private.file_name().unwrap(), &opened, &quarantine)
                .unwrap_err();

        assert!(format!("{error:#}").contains(&quarantine.display().to_string()));
        assert!(quarantine.is_dir());
        assert!(displaced.is_dir());
    }

    #[test]
    fn private_loser_cleanup_preserves_foreign_replacement_before_delete() {
        let temp = tempfile::tempdir().unwrap();
        let private = temp.path().join(".skill-bootstrap-owned");
        let quarantine = temp.path().join(".skill-cleanup-quarantine-test");
        let owned_evidence = temp.path().join("owned-evidence");
        std::fs::create_dir(&private).unwrap();
        let opened = std::fs::File::open(&private).unwrap();
        let parent = std::fs::File::open(temp.path()).unwrap();
        let quarantined =
            quarantine_named_directory(&parent, private.file_name().unwrap(), &opened, &quarantine)
                .unwrap();
        std::fs::rename(&quarantine, &owned_evidence).unwrap();
        std::fs::create_dir(&quarantine).unwrap();

        let error = remove_quarantined_directory(quarantined).unwrap_err();

        assert!(format!("{error:#}").contains(&quarantine.display().to_string()));
        assert!(quarantine.is_dir());
        assert!(owned_evidence.is_dir());
    }
}
