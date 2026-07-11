use crate::skill_source::canonical_relative_path;
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;
use std::path::Path;

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

#[cfg(unix)]
pub(crate) async fn open_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<(tokio::fs::File, u64, u32)> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, openat};
    use std::fs::File;
    let (parent, name) = open_parent(root, relative)?;
    let descriptor = openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!("prepared package source is not a regular file");
    }
    if stat.st_nlink != 1 {
        anyhow::bail!("prepared package source must not be a hard link");
    }
    Ok((
        tokio::fs::File::from_std(File::from(descriptor)),
        u64::try_from(stat.st_size).context("package file has negative size")?,
        u32::from(stat.st_mode) & 0o777,
    ))
}

#[cfg(windows)]
pub(crate) async fn open_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<(tokio::fs::File, u64, u32)> {
    canonical_relative_path(relative)?;
    let (file, length) = crate::skill_store_windows::open_regular_file_beneath(
        root.windows_descriptor(),
        relative,
        false,
        false,
    )?;
    Ok((tokio::fs::File::from_std(file), length, 0o644))
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) async fn open_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
) -> anyhow::Result<(tokio::fs::File, u64, u32)> {
    canonical_relative_path(relative)?;
    root.verify()?;
    let path = root.path().join(relative);
    let file = tokio::fs::File::open(&path).await?;
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        anyhow::bail!("prepared package source is not a regular file");
    }
    root.verify()?;
    Ok((file, metadata.len(), 0o644))
}

#[cfg(unix)]
pub(crate) async fn create_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
    mode: u32,
) -> anyhow::Result<tokio::fs::File> {
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, openat};
    use std::fs::File;
    let (parent, name) = open_parent(root, relative)?;
    let descriptor = openat(
        &parent,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(mode & 0o777)?),
    )?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!("prepared package destination is not a regular file");
    }
    Ok(tokio::fs::File::from_std(File::from(descriptor)))
}

#[cfg(windows)]
pub(crate) async fn create_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
    _mode: u32,
) -> anyhow::Result<tokio::fs::File> {
    canonical_relative_path(relative)?;
    let (file, _) = crate::skill_store_windows::open_regular_file_beneath(
        root.windows_descriptor(),
        relative,
        true,
        true,
    )?;
    Ok(tokio::fs::File::from_std(file))
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) async fn create_regular_file(
    root: &PreparedStoreDirectory,
    relative: &Path,
    _mode: u32,
) -> anyhow::Result<tokio::fs::File> {
    canonical_relative_path(relative)?;
    root.verify()?;
    let file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(root.path().join(relative))
        .await?;
    root.verify()?;
    Ok(file)
}

#[cfg(unix)]
fn open_mode_target(
    root: &PreparedStoreDirectory,
    relative: Option<&Path>,
    directory: bool,
) -> anyhow::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, openat};
    let flags = OFlags::RDONLY
        | OFlags::CLOEXEC
        | OFlags::NOFOLLOW
        | if directory {
            OFlags::DIRECTORY
        } else {
            OFlags::empty()
        };
    match relative {
        None => Ok(rustix::io::dup(root.descriptor())?),
        Some(relative) => {
            let (parent, name) = open_parent(root, relative)?;
            Ok(openat(&parent, name, flags, Mode::empty())?)
        }
    }
}

#[cfg(unix)]
pub(crate) async fn set_mode(
    root: &PreparedStoreDirectory,
    relative: Option<&Path>,
    mode: u32,
    directory: bool,
) -> anyhow::Result<()> {
    use rustix::fs::{Mode, RawMode, fchmod};
    let descriptor = open_mode_target(root, relative, directory)?;
    fchmod(
        descriptor,
        Mode::from_raw_mode(RawMode::try_from(mode & 0o777)?),
    )?;
    Ok(())
}

#[cfg(windows)]
pub(crate) async fn set_mode(
    root: &PreparedStoreDirectory,
    relative: Option<&Path>,
    _mode: u32,
    directory: bool,
) -> anyhow::Result<()> {
    crate::skill_store_windows::validate_target_beneath(
        root.windows_descriptor(),
        relative,
        directory,
    )
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) async fn set_mode(
    root: &PreparedStoreDirectory,
    _relative: Option<&Path>,
    _mode: u32,
    _directory: bool,
) -> anyhow::Result<()> {
    root.verify()
}

pub(crate) async fn set_readonly(
    root: &PreparedStoreDirectory,
    relative: Option<&Path>,
    directory: bool,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, RawMode, fchmod, fstat};
        let descriptor = open_mode_target(root, relative, directory)?;
        let mode = (u32::from(fstat(&descriptor)?.st_mode) & !0o222) & 0o777;
        fchmod(descriptor, Mode::from_raw_mode(RawMode::try_from(mode)?))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let (descriptor, target) = windows_attribute_target(root, relative);
        crate::skill_store_windows::set_readonly_beneath(descriptor, target, directory, true)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        root.verify()?;
        let path =
            relative.map_or_else(|| root.path().to_path_buf(), |path| root.path().join(path));
        let mut permissions = tokio::fs::metadata(&path).await?.permissions();
        permissions.set_readonly(true);
        tokio::fs::set_permissions(path, permissions).await?;
        root.verify()
    }
}

pub(crate) async fn set_writable(
    root: &PreparedStoreDirectory,
    relative: Option<&Path>,
    directory: bool,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, RawMode, fchmod, fstat};
        let descriptor = open_mode_target(root, relative, directory)?;
        let access = if directory { 0o700 } else { 0o600 };
        let mode = (u32::from(fstat(&descriptor)?.st_mode) | access) & 0o777;
        fchmod(descriptor, Mode::from_raw_mode(RawMode::try_from(mode)?))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        let (descriptor, target) = windows_attribute_target(root, relative);
        crate::skill_store_windows::set_readonly_beneath(descriptor, target, directory, false)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        root.verify()?;
        let path =
            relative.map_or_else(|| root.path().to_path_buf(), |path| root.path().join(path));
        let mut permissions = tokio::fs::metadata(&path).await?.permissions();
        permissions.set_readonly(false);
        tokio::fs::set_permissions(path, permissions).await?;
        root.verify()
    }
}

#[cfg(windows)]
fn windows_attribute_target<'a>(
    root: &'a PreparedStoreDirectory,
    relative: Option<&'a Path>,
) -> (&'a std::fs::File, Option<&'a Path>) {
    match relative {
        Some(relative) => (root.windows_descriptor(), Some(relative)),
        None => (root.windows_descriptor(), None),
    }
}
