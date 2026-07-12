use crate::skill_source::canonical_relative_path;
use crate::skill_store_fs_types::PackageLimits;
use crate::skill_store_locks::StoreRootIdentity;
use crate::skill_store_secure_snapshot::{SecurePackageSnapshot, SecureTreeSnapshot};
use anyhow::Context;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct PreparedStoreDirectory {
    root: StoreRootIdentity,
    relative: std::path::PathBuf,
    path: std::path::PathBuf,
    identity: Arc<same_file::Handle>,
    #[cfg(unix)]
    descriptor: Arc<File>,
    #[cfg(windows)]
    descriptor: Arc<File>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedStoreChild {
    pub(crate) name: String,
    pub(crate) directory: PreparedStoreDirectory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedStoreUnknownKind {
    Symlink,
    RegularFile,
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedStoreUnknown {
    pub(crate) name: String,
    pub(crate) kind: PreparedStoreUnknownKind,
}

pub(crate) struct PreparedStoreListing {
    pub(crate) children: Vec<PreparedStoreChild>,
    pub(crate) unknown: Vec<PreparedStoreUnknown>,
    pub(crate) observed: usize,
    pub(crate) exceeded: bool,
}

impl PreparedStoreDirectory {
    pub(crate) fn open(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<Self> {
        let descriptor = open_prepared_directory_platform(root, relative)?;
        Self::from_opened(root, relative, descriptor)
    }

    pub(crate) fn from_opened(
        root: &StoreRootIdentity,
        relative: &Path,
        descriptor: File,
    ) -> anyhow::Result<Self> {
        let identity = same_file::Handle::from_file(descriptor.try_clone()?)?;
        let directory = Self {
            root: root.clone(),
            relative: relative.to_path_buf(),
            path: root.path().join(relative),
            identity: Arc::new(identity),
            #[cfg(any(unix, windows))]
            descriptor: Arc::new(descriptor),
        };
        directory.verify()?;
        Ok(directory)
    }

    pub(crate) fn verify(&self) -> anyhow::Result<()> {
        let descriptor = open_verification_directory_platform(&self.root, &self.relative)?;
        let current = same_file::Handle::from_file(descriptor)?;
        if current != *self.identity {
            anyhow::bail!(
                "prepared store revision identity changed: {}",
                self.path.display()
            );
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn root_identity(&self) -> &StoreRootIdentity {
        &self.root
    }

    #[cfg(any(unix, windows))]
    pub(crate) fn capture_identity(&self) -> anyhow::Result<StoreRootIdentity> {
        StoreRootIdentity::capture_opened(self.path.clone(), self.descriptor.as_ref())
    }

    #[cfg(all(not(unix), not(windows)))]
    pub(crate) fn capture_identity(&self) -> anyhow::Result<StoreRootIdentity> {
        StoreRootIdentity::capture(self.path.clone())
    }

    pub(crate) fn relative(&self) -> &Path {
        &self.relative
    }

    #[cfg(unix)]
    pub(crate) fn descriptor(&self) -> &File {
        &self.descriptor
    }

    #[cfg(windows)]
    pub(crate) fn windows_descriptor(&self) -> &File {
        &self.descriptor
    }
}

pub(crate) async fn ensure_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || ensure_directory_platform(&root, &relative))
        .await
        .context("prepared-root directory worker failed")?
}

pub(crate) async fn reserve_opened_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<PreparedStoreDirectory> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || {
        reserve_directory_platform(&root, &relative)?;
        PreparedStoreDirectory::open(&root, &relative)
    })
    .await
    .context("prepared-root opened reservation worker failed")?
}

pub(crate) async fn open_prepared_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<PreparedStoreDirectory> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || PreparedStoreDirectory::open(&root, &relative))
        .await
        .context("prepared-root open worker failed")?
}

pub(crate) async fn remove_opened_tree(directory: &PreparedStoreDirectory) -> anyhow::Result<()> {
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || remove_opened_tree_platform(&directory))
        .await
        .context("prepared-root opened cleanup worker failed")?
}

pub(crate) async fn list_opened_root_directories(
    root: &StoreRootIdentity,
    max_entries: usize,
) -> anyhow::Result<PreparedStoreListing> {
    root.verify("store enumeration root")?;
    let mut entries = tokio::fs::read_dir(root.path()).await?;
    let mut children = Vec::new();
    let mut unknown = Vec::new();
    let mut observed = 0usize;
    while let Some(entry) = entries.next_entry().await? {
        if observed == max_entries {
            root.verify("store enumeration root")?;
            return Ok(PreparedStoreListing {
                children,
                unknown,
                observed,
                exceeded: true,
            });
        }
        observed = observed
            .checked_add(1)
            .context("store enumeration entry count overflow")?;
        let file_type = entry.file_type().await?;
        if file_type.is_symlink() || !file_type.is_dir() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("store entry name is not UTF-8"))?;
            unknown.push(PreparedStoreUnknown {
                name,
                kind: if file_type.is_symlink() {
                    PreparedStoreUnknownKind::Symlink
                } else if file_type.is_file() {
                    PreparedStoreUnknownKind::RegularFile
                } else {
                    PreparedStoreUnknownKind::Other
                },
            });
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("store entry name is not UTF-8"))?;
        let relative = std::path::PathBuf::from(&name);
        canonical_relative_path(&relative)?;
        let directory = open_prepared_directory(root, &relative).await?;
        children.push(PreparedStoreChild { name, directory });
    }
    root.verify("store enumeration root")?;
    children.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PreparedStoreListing {
        children,
        unknown,
        observed,
        exceeded: false,
    })
}

pub(crate) async fn list_opened_child_directories(
    parent: &PreparedStoreDirectory,
    max_entries: usize,
) -> anyhow::Result<PreparedStoreListing> {
    parent.verify()?;
    let mut entries = tokio::fs::read_dir(parent.path()).await?;
    let mut children = Vec::new();
    let mut unknown = Vec::new();
    let mut observed = 0usize;
    while let Some(entry) = entries.next_entry().await? {
        if observed == max_entries {
            parent.verify()?;
            return Ok(PreparedStoreListing {
                children,
                unknown,
                observed,
                exceeded: true,
            });
        }
        observed = observed
            .checked_add(1)
            .context("store enumeration entry count overflow")?;
        let file_type = entry.file_type().await?;
        if file_type.is_symlink() || !file_type.is_dir() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("store entry name is not UTF-8"))?;
            unknown.push(PreparedStoreUnknown {
                name,
                kind: if file_type.is_symlink() {
                    PreparedStoreUnknownKind::Symlink
                } else if file_type.is_file() {
                    PreparedStoreUnknownKind::RegularFile
                } else {
                    PreparedStoreUnknownKind::Other
                },
            });
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("store entry name is not UTF-8"))?;
        let relative_name = std::path::PathBuf::from(&name);
        canonical_relative_path(&relative_name)?;
        let relative = parent.relative.join(&relative_name);
        let directory = open_prepared_directory(&parent.root, &relative).await?;
        children.push(PreparedStoreChild { name, directory });
    }
    parent.verify()?;
    children.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PreparedStoreListing {
        children,
        unknown,
        observed,
        exceeded: false,
    })
}

pub(crate) async fn ensure_opened_child_directory(
    directory: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let directory = directory.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || {
        ensure_opened_child_directory_platform(&directory, &relative)
    })
    .await
    .context("prepared-root child directory worker failed")?
}

pub(crate) async fn package_snapshot(
    root: &StoreRootIdentity,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    canonical_relative_path(relative)?;
    let root = root.clone();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || snapshot_platform(&root, &relative, limits))
        .await
        .context("prepared-root snapshot worker failed")?
}

pub(crate) async fn opened_package_snapshot(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || opened_package_snapshot_platform(&directory, limits))
        .await
        .context("prepared-root opened snapshot worker failed")?
}

pub(crate) async fn opened_tree_snapshot(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || opened_tree_snapshot_platform(&directory, limits))
        .await
        .context("prepared-root opened tree snapshot worker failed")?
}

#[cfg(unix)]
fn duplicate_root(root: &StoreRootIdentity) -> anyhow::Result<std::os::fd::OwnedFd> {
    Ok(rustix::io::dup(root.descriptor())?)
}

#[cfg(unix)]
fn open_directory(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, openat};
    let mut directory = duplicate_root(root)?;
    for component in relative.components() {
        directory = openat(
            &directory,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<File> {
    Ok(File::from(open_directory(root, relative)?))
}

#[cfg(unix)]
fn open_prepared_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    open_directory_platform(root, relative)
}

#[cfg(unix)]
fn open_verification_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    open_directory_platform(root, relative)
}

#[cfg(windows)]
fn open_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<File> {
    let (file, _, _) = crate::skill_store_windows::open_directory_beneath(
        root.windows_descriptor(),
        root.windows_identity(),
        relative,
    )?;
    Ok(file)
}

#[cfg(windows)]
fn open_prepared_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    let (file, _, _) = crate::skill_store_windows::open_mutable_directory_beneath(
        root.windows_descriptor(),
        root.windows_identity(),
        relative,
    )?;
    Ok(file)
}

#[cfg(windows)]
fn open_verification_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    crate::skill_store_windows::open_verification_directory_beneath(
        root.windows_descriptor(),
        root.windows_identity(),
        relative,
    )
}

#[cfg(all(not(unix), not(windows)))]
fn open_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<File> {
    root.verify("store")?;
    Ok(File::open(root.path().join(relative))?)
}

#[cfg(all(not(unix), not(windows)))]
fn open_prepared_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    open_directory_platform(root, relative)
}

#[cfg(all(not(unix), not(windows)))]
fn open_verification_directory_platform(
    root: &StoreRootIdentity,
    relative: &Path,
) -> anyhow::Result<File> {
    open_directory_platform(root, relative)
}

#[cfg(unix)]
fn ensure_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut directory = duplicate_root(root)?;
    for component in relative.components() {
        let name = component.as_os_str();
        match mkdirat(
            &directory,
            name,
            Mode::from_raw_mode(RawMode::try_from(0o755_u32)?),
        ) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        directory = openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    Ok(())
}

#[cfg(unix)]
fn reserve_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut parent = duplicate_root(root)?;
    let components = relative.components().collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .context("reservation path is empty")?;
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
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::EXIST) => anyhow::bail!(
            "skill store destination already exists: {}",
            root.path().join(relative).display()
        ),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn remove_opened_tree_platform(directory: &PreparedStoreDirectory) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};
    let opened = rustix::io::dup(directory.descriptor())?;
    remove_contents(&opened)?;
    directory.verify()?;
    let mut parent = duplicate_root(&directory.root)?;
    let components = directory.relative.components().collect::<Vec<_>>();
    let (name, parents) = components.split_last().context("cleanup path is empty")?;
    for component in parents {
        parent = openat(
            &parent,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    unlinkat(&parent, name.as_os_str(), AtFlags::REMOVEDIR)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_opened_child_directory_platform(
    directory: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut current = rustix::io::dup(directory.descriptor())?;
    for component in relative.components() {
        let name = component.as_os_str();
        match mkdirat(
            &current,
            name,
            Mode::from_raw_mode(RawMode::try_from(0o755_u32)?),
        ) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        current = openat(
            &current,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_opened_child_directory_platform(
    directory: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<()> {
    let mut parent = directory.windows_descriptor().try_clone()?;
    let mut parent_identity = crate::skill_store_windows::identity_for_file(&parent)?;
    for component in relative.components() {
        let (child, child_identity, _, _) =
            crate::skill_store_windows::create_or_open_directory_child(
                &parent,
                parent_identity,
                component.as_os_str(),
            )?;
        parent = child;
        parent_identity = child_identity;
    }
    directory.verify()
}

#[cfg(all(not(unix), not(windows)))]
fn ensure_opened_child_directory_platform(
    directory: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<()> {
    directory.verify()?;
    std::fs::create_dir_all(directory.path().join(relative))?;
    directory.verify()
}

#[cfg(windows)]
fn remove_opened_tree_platform(directory: &PreparedStoreDirectory) -> anyhow::Result<()> {
    directory.verify()?;
    crate::skill_store_windows::delete_opened_tree(directory.windows_descriptor())
}

#[cfg(all(not(unix), not(windows)))]
fn remove_opened_tree_platform(directory: &PreparedStoreDirectory) -> anyhow::Result<()> {
    directory.verify()?;
    std::fs::remove_dir_all(directory.path())?;
    Ok(())
}

#[cfg(unix)]
fn remove_contents(directory: &std::os::fd::OwnedFd) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Dir, Mode, OFlags, openat, unlinkat};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let name = OsStr::from_bytes(bytes);
        match openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(child) => {
                remove_contents(&child)?;
                unlinkat(directory, name, AtFlags::REMOVEDIR)?;
            }
            Err(rustix::io::Errno::NOTDIR) | Err(rustix::io::Errno::LOOP) => {
                unlinkat(directory, name, AtFlags::empty())?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn snapshot_platform(
    root: &StoreRootIdentity,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let directory = open_directory(root, relative)?;
    crate::skill_store_secure_fs::snapshot_opened(&root.path().join(relative), directory, limits)
}

#[cfg(unix)]
fn opened_package_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let descriptor = rustix::io::dup(directory.descriptor())?;
    crate::skill_store_secure_fs::snapshot_opened(directory.path(), descriptor, limits)
}

#[cfg(unix)]
fn opened_tree_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    let descriptor = rustix::io::dup(directory.descriptor())?;
    crate::skill_store_secure_fs::scan_opened(directory.path(), descriptor, limits)
}

#[cfg(windows)]
fn opened_package_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let path = crate::skill_store_windows::final_path_for_file(directory.windows_descriptor())?;
    let descriptor = directory.windows_descriptor().try_clone()?;
    crate::skill_store_secure_fs::snapshot_windows_opened(&path, descriptor, limits)
}

#[cfg(windows)]
fn opened_tree_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    let descriptor = directory.windows_descriptor().try_clone()?;
    crate::skill_store_secure_fs::tree_windows_opened(descriptor, limits)
}

#[cfg(all(not(unix), not(windows)))]
fn opened_package_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    directory.verify()?;
    crate::skill_store_secure_fs::snapshot_beneath(directory.path(), Path::new(""), limits)
}

#[cfg(all(not(unix), not(windows)))]
fn opened_tree_snapshot_platform(
    directory: &PreparedStoreDirectory,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    directory.verify()?;
    crate::skill_store_secure_fs::tree_direct(directory.path(), limits)
}

#[cfg(windows)]
fn ensure_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    let mut parent = root.windows_descriptor().try_clone()?;
    let mut parent_identity = root.windows_identity();
    for component in relative.components() {
        let (child, child_identity, _, _) =
            crate::skill_store_windows::create_or_open_directory_child(
                &parent,
                parent_identity,
                component.as_os_str(),
            )?;
        parent = child;
        parent_identity = child_identity;
    }
    root.verify("store")
}

#[cfg(windows)]
fn reserve_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let (parent_handle, parent_identity, _) = crate::skill_store_windows::open_directory_beneath(
        root.windows_descriptor(),
        root.windows_identity(),
        parent,
    )?;
    let name = relative.file_name().context("reservation path is empty")?;
    let (_, _, _, created) = crate::skill_store_windows::create_or_open_directory_child(
        &parent_handle,
        parent_identity,
        name,
    )?;
    if !created {
        anyhow::bail!(
            "skill store destination already exists: {}",
            root.path().join(relative).display()
        );
    }
    root.verify("store")
}

#[cfg(windows)]
fn snapshot_platform(
    root: &StoreRootIdentity,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let (directory, _, final_path) = crate::skill_store_windows::open_directory_beneath(
        root.windows_descriptor(),
        root.windows_identity(),
        relative,
    )?;
    crate::skill_store_secure_fs::snapshot_windows_opened(&final_path, directory, limits)
}

#[cfg(all(not(unix), not(windows)))]
fn ensure_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    root.verify("store")?;
    std::fs::create_dir_all(root.path().join(relative))?;
    root.verify("store")
}

#[cfg(all(not(unix), not(windows)))]
fn reserve_directory_platform(root: &StoreRootIdentity, relative: &Path) -> anyhow::Result<()> {
    root.verify("store")?;
    std::fs::create_dir(root.path().join(relative))?;
    root.verify("store")
}

#[cfg(all(not(unix), not(windows)))]
fn snapshot_platform(
    root: &StoreRootIdentity,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    root.verify("store")?;
    crate::skill_store_secure_fs::snapshot_beneath(root.path(), relative, limits)
}
