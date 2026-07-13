use super::{AttemptPathKind, AttemptRecord, ObjectBinding};
use anyhow::Context;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const JOURNAL_LIMIT: u64 = 256 * 1024;

pub(super) fn canonical_real_directory(path: &Path) -> anyhow::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)?;
    validate_metadata(&metadata, AttemptPathKind::Directory)?;
    Ok(std::fs::canonicalize(path)?)
}

pub(super) fn create_private_object(path: &Path, kind: AttemptPathKind) -> anyhow::Result<()> {
    match kind {
        AttemptPathKind::File => {
            create_file_nofollow(path)?;
        }
        AttemptPathKind::Directory => {
            std::fs::create_dir(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    Ok(())
}

pub(super) fn write_object_binding(
    file: &File,
    path: &Path,
    binding: &ObjectBinding,
    replace: bool,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(binding)?;
    #[cfg(unix)]
    {
        use rustix::fs::{XattrFlags, fsetxattr};
        let flags = if replace {
            XattrFlags::REPLACE
        } else {
            XattrFlags::CREATE
        };
        fsetxattr(file, object_binding_name(), &bytes, flags)?;
    }
    #[cfg(windows)]
    {
        let mut options = OpenOptions::new();
        options.write(true).create(!replace).truncate(true);
        let mut stream = options.open(object_binding_stream(path))?;
        stream.write_all(&bytes)?;
        stream.sync_all()?;
    }
    #[cfg(all(not(unix), not(windows)))]
    anyhow::bail!("tenant object bindings are unsupported on this platform");
    let _ = path;
    Ok(())
}

pub(super) fn read_object_binding(file: &File, _path: &Path) -> anyhow::Result<ObjectBinding> {
    #[cfg(unix)]
    let bytes = {
        let mut bytes = [0_u8; 4096];
        let length = rustix::fs::fgetxattr(file, object_binding_name(), &mut bytes)?;
        bytes[..length].to_vec()
    };
    #[cfg(windows)]
    let bytes = std::fs::read(object_binding_stream(_path))?;
    #[cfg(all(not(unix), not(windows)))]
    anyhow::bail!("tenant object bindings are unsupported on this platform");
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(all(unix, target_vendor = "apple"))]
fn object_binding_name() -> &'static str {
    "com.generalagent.tenant-attempt"
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn object_binding_name() -> &'static str {
    "user.general-agent-tenant-attempt"
}

#[cfg(windows)]
fn object_binding_stream(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}:general-agent-tenant-attempt", path.display()))
}

pub(super) fn open_nofollow(
    path: &Path,
    kind: AttemptPathKind,
    write: bool,
) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    if kind == AttemptPathKind::File {
        options.write(write);
    }
    set_nofollow(&mut options, kind);
    let file = options.open(path)?;
    validate_metadata(&file.metadata()?, kind)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(file)
}

fn create_file_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    set_nofollow(&mut options, AttemptPathKind::File);
    options.open(path)
}

#[cfg(unix)]
fn set_nofollow(options: &mut OpenOptions, _kind: AttemptPathKind) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
}

#[cfg(windows)]
fn set_nofollow(options: &mut OpenOptions, kind: AttemptPathKind) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let directory = u32::from(kind == AttemptPathKind::Directory) * FILE_FLAG_BACKUP_SEMANTICS;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | directory);
}

#[cfg(all(not(unix), not(windows)))]
fn set_nofollow(_options: &mut OpenOptions, _kind: AttemptPathKind) {}

pub(super) fn validate_metadata(
    metadata: &std::fs::Metadata,
    kind: AttemptPathKind,
) -> anyhow::Result<()> {
    let valid = !metadata.file_type().is_symlink()
        && match kind {
            AttemptPathKind::File => metadata.is_file(),
            AttemptPathKind::Directory => metadata.is_dir(),
        };
    anyhow::ensure!(valid, "tenant attempt path has an invalid type");
    validate_link_count(metadata, kind)
}

pub(super) fn validate_link_count(
    metadata: &std::fs::Metadata,
    kind: AttemptPathKind,
) -> anyhow::Result<()> {
    if kind != AttemptPathKind::File {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "tenant attempt file must have one link"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            metadata.number_of_links() == 1,
            "tenant attempt file must have one link"
        );
    }
    Ok(())
}

pub(super) fn write_record(path: &Path, record: &AttemptRecord) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    anyhow::ensure!(
        bytes.len() as u64 <= JOURNAL_LIMIT,
        "tenant journal is too large"
    );
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .context("tenant journal has no file name")?
            .to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut file = create_file_nofollow(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    atomic_replace(&temporary, path)?;
    sync_directory(path.parent().context("tenant journal has no parent")?)?;
    Ok(())
}

pub(super) fn read_record(path: &Path) -> anyhow::Result<AttemptRecord> {
    let file = open_nofollow(path, AttemptPathKind::File, false)?;
    anyhow::ensure!(
        file.metadata()?.len() <= JOURNAL_LIMIT,
        "tenant journal is too large"
    );
    let mut bytes = Vec::new();
    file.take(JOURNAL_LIMIT + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= JOURNAL_LIMIT,
        "tenant journal is too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) fn sync_directory(path: &Path) -> anyhow::Result<()> {
    let directory = open_nofollow(path, AttemptPathKind::Directory, false)?;
    directory.sync_all()?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn rename_noreplace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    use rustix::fs::{RenameFlags, renameat_with};
    let source_parent = open_nofollow(
        source.parent().context("tenant source has no parent")?,
        AttemptPathKind::Directory,
        false,
    )?;
    let destination_parent = open_nofollow(
        destination
            .parent()
            .context("tenant destination has no parent")?,
        AttemptPathKind::Directory,
        false,
    )?;
    renameat_with(
        &source_parent,
        source.file_name().context("tenant source has no name")?,
        &destination_parent,
        destination
            .file_name()
            .context("tenant destination has no name")?,
        RenameFlags::NOREPLACE,
    )?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn rename_noreplace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    windows_move(source, destination, false)
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn rename_noreplace(_source: &Path, _destination: &Path) -> anyhow::Result<()> {
    anyhow::bail!("atomic no-replace rename is unsupported on this platform")
}

#[cfg(unix)]
fn atomic_replace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    std::fs::rename(source, destination)?;
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> anyhow::Result<()> {
    windows_move(source, destination, true)
}

#[cfg(all(not(unix), not(windows)))]
fn atomic_replace(_source: &Path, _destination: &Path) -> anyhow::Result<()> {
    anyhow::bail!("atomic journal replacement is unsupported on this platform")
}

#[cfg(windows)]
fn windows_move(source: &Path, destination: &Path, replace: bool) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    let result = unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) };
    anyhow::ensure!(
        result != 0,
        "Windows tenant rename failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(unix)]
pub(super) fn remove_private_object(
    path: &Path,
    kind: AttemptPathKind,
    expected: &File,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, fstat, statat, unlinkat};
    let parent = open_nofollow(
        path.parent().context("tenant quarantine has no parent")?,
        AttemptPathKind::Directory,
        false,
    )?;
    let name = path.file_name().context("tenant quarantine has no name")?;
    let opened = fstat(expected)?;
    let current = statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
    anyhow::ensure!(
        opened.st_dev == current.st_dev && opened.st_ino == current.st_ino,
        "tenant quarantine changed before unlink"
    );
    let flags = if kind == AttemptPathKind::Directory {
        AtFlags::REMOVEDIR
    } else {
        AtFlags::empty()
    };
    unlinkat(&parent, name, flags)?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn remove_private_object(
    _path: &Path,
    _kind: AttemptPathKind,
    expected: &File,
) -> anyhow::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_INFO, FileDispositionInfo, SetFileInformationByHandle,
    };
    let information = FILE_DISPOSITION_INFO { DeleteFile: true };
    let result = unsafe {
        SetFileInformationByHandle(
            expected.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            std::ptr::from_ref(&information).cast(),
            u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>())?,
        )
    };
    anyhow::ensure!(
        result != 0,
        "Windows tenant handle-bound deletion failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn remove_private_object(
    _path: &Path,
    _kind: AttemptPathKind,
    _expected: &File,
) -> anyhow::Result<()> {
    anyhow::bail!("identity-bound cleanup is unsupported on this platform")
}

#[cfg(test)]
pub(super) fn replace_quarantine_for_test(
    path: &Path,
    kind: AttemptPathKind,
) -> anyhow::Result<()> {
    let displaced = path.with_extension("displaced-owned");
    rename_noreplace(path, &displaced)?;
    match kind {
        AttemptPathKind::File => std::fs::write(path, b"foreign replacement")?,
        AttemptPathKind::Directory => std::fs::create_dir(path)?,
    }
    Ok(())
}
