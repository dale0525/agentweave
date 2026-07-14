use super::{AttemptPathKind, AttemptRecord, ObjectBinding};
#[cfg(windows)]
use super::{windows_link_count_is_one, windows_number_of_links};
use anyhow::Context;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const JOURNAL_LIMIT: u64 = 256 * 1024;
#[cfg(any(windows, test))]
const WINDOWS_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0002;
#[cfg(any(windows, test))]
const WINDOWS_DIRECTORY_FLAGS: u32 = 0x0200_0000 | 0x0020_0000;
#[cfg(any(windows, test))]
const WINDOWS_DIRECTORY_SYNC_ACCESS: u32 = 0x4000_0000;
#[cfg(any(windows, test))]
const WINDOWS_DIRECTORY_WRITE_ACCESS: u32 = 0x8000_0000 | 0x4000_0000;
#[cfg(any(windows, test))]
const WINDOWS_CLEANUP_ACCESS: u32 = 0x8000_0000 | 0x0001_0000;

#[cfg(test)]
pub(super) struct WindowsOpenContract {
    pub(super) share_mode: u32,
    pub(super) directory_flags: u32,
    pub(super) directory_sync_access: u32,
    pub(super) directory_write_access: u32,
    pub(super) cleanup_access: u32,
}

#[cfg(test)]
pub(super) fn windows_open_contract_for_test() -> WindowsOpenContract {
    WindowsOpenContract {
        share_mode: WINDOWS_SHARE_MODE,
        directory_flags: WINDOWS_DIRECTORY_FLAGS,
        directory_sync_access: WINDOWS_DIRECTORY_SYNC_ACCESS,
        directory_write_access: WINDOWS_DIRECTORY_WRITE_ACCESS,
        cleanup_access: WINDOWS_CLEANUP_ACCESS,
    }
}

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
    _file: &File,
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
        fsetxattr(_file, object_binding_name(), &bytes, flags)?;
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

pub(super) enum ObjectBindingStatus {
    Absent,
    Malformed,
    Valid(ObjectBinding),
}

pub(super) fn read_object_binding(file: &File, path: &Path) -> anyhow::Result<ObjectBinding> {
    match inspect_object_binding(file, path)? {
        ObjectBindingStatus::Valid(binding) => Ok(binding),
        ObjectBindingStatus::Absent => anyhow::bail!("tenant object binding is absent"),
        ObjectBindingStatus::Malformed => anyhow::bail!("tenant object binding is malformed"),
    }
}

#[cfg(unix)]
pub(super) fn inspect_object_binding(
    file: &File,
    _path: &Path,
) -> anyhow::Result<ObjectBindingStatus> {
    let mut bytes = [0_u8; 4096];
    let length = match rustix::fs::fgetxattr(file, object_binding_name(), &mut bytes) {
        Ok(length) => length,
        Err(error) if missing_object_binding(error) => return Ok(ObjectBindingStatus::Absent),
        Err(error) => return Err(error.into()),
    };
    Ok(decode_object_binding(&bytes[..length]))
}

#[cfg(windows)]
pub(super) fn inspect_object_binding(
    _file: &File,
    path: &Path,
) -> anyhow::Result<ObjectBindingStatus> {
    let bytes = match std::fs::read(object_binding_stream(path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ObjectBindingStatus::Absent);
        }
        Err(error) => return Err(error.into()),
    };
    Ok(decode_object_binding(&bytes))
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn inspect_object_binding(
    _file: &File,
    _path: &Path,
) -> anyhow::Result<ObjectBindingStatus> {
    anyhow::bail!("tenant object bindings are unsupported on this platform")
}

fn decode_object_binding(bytes: &[u8]) -> ObjectBindingStatus {
    match serde_json::from_slice(bytes) {
        Ok(binding) => ObjectBindingStatus::Valid(binding),
        Err(_) => ObjectBindingStatus::Malformed,
    }
}

#[cfg(all(unix, target_vendor = "apple"))]
fn missing_object_binding(error: rustix::io::Errno) -> bool {
    matches!(error, rustix::io::Errno::NOATTR | rustix::io::Errno::NODATA)
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn missing_object_binding(error: rustix::io::Errno) -> bool {
    error == rustix::io::Errno::NODATA
}

#[cfg(all(unix, target_vendor = "apple"))]
fn object_binding_name() -> &'static str {
    "com.agentweave.tenant-attempt"
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn object_binding_name() -> &'static str {
    "user.agentweave-tenant-attempt"
}

#[cfg(windows)]
fn object_binding_stream(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}:agentweave-tenant-attempt", path.display()))
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
    #[cfg(windows)]
    if kind == AttemptPathKind::Directory && write {
        use std::os::windows::fs::OpenOptionsExt;
        options.access_mode(WINDOWS_DIRECTORY_WRITE_ACCESS);
    }
    set_nofollow(&mut options, kind);
    let file = options.open(path)?;
    validate_opened_file(&file, kind)?;
    Ok(file)
}

#[cfg(windows)]
pub(super) fn open_delete_nofollow(path: &Path, kind: AttemptPathKind) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options
        .access_mode(WINDOWS_CLEANUP_ACCESS)
        .share_mode(WINDOWS_SHARE_MODE);
    set_nofollow(&mut options, kind);
    let file = options.open(path)?;
    validate_opened_file(&file, kind)?;
    Ok(file)
}

#[cfg(not(windows))]
pub(super) fn open_delete_nofollow(path: &Path, kind: AttemptPathKind) -> std::io::Result<File> {
    open_nofollow(path, kind, false)
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
    let directory = u32::from(kind == AttemptPathKind::Directory) * 0x0200_0000;
    options
        .share_mode(WINDOWS_SHARE_MODE)
        .custom_flags(0x0020_0000 | directory);
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
    _metadata: &std::fs::Metadata,
    kind: AttemptPathKind,
) -> anyhow::Result<()> {
    if kind != AttemptPathKind::File {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            _metadata.nlink() == 1,
            "tenant attempt file must have one link"
        );
    }
    Ok(())
}

fn validate_opened_file(file: &File, kind: AttemptPathKind) -> std::io::Result<()> {
    validate_metadata(&file.metadata()?, kind)
        .and_then(|()| validate_opened_link_count(file, kind))
        .map_err(|error| std::io::Error::other(error.to_string()))
}

pub(super) fn validate_opened_link_count(file: &File, kind: AttemptPathKind) -> anyhow::Result<()> {
    if kind != AttemptPathKind::File {
        return Ok(());
    }
    #[cfg(windows)]
    anyhow::ensure!(
        windows_link_count_is_one(windows_number_of_links(file)?),
        "tenant attempt file must have one link"
    );
    #[cfg(not(windows))]
    validate_link_count(&file.metadata()?, kind)?;
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

#[cfg(not(windows))]
pub(super) fn sync_directory(path: &Path) -> anyhow::Result<()> {
    let directory = open_nofollow(path, AttemptPathKind::Directory, false)?;
    directory.sync_all()?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn sync_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options
        .access_mode(WINDOWS_DIRECTORY_SYNC_ACCESS)
        .share_mode(WINDOWS_SHARE_MODE)
        .custom_flags(WINDOWS_DIRECTORY_FLAGS);
    let directory = options.open(path)?;
    validate_metadata(&directory.metadata()?, AttemptPathKind::Directory)?;
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

#[cfg(unix)]
pub(super) fn clear_owned_directory_contents(directory: &File, _path: &Path) -> anyhow::Result<()> {
    let opened = rustix::io::dup(directory)?;
    let mut entries = 0_usize;
    clear_opened_directory(&opened, &mut entries)
}

#[cfg(unix)]
fn clear_opened_directory(
    directory: &std::os::fd::OwnedFd,
    entries: &mut usize,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Dir, Mode, OFlags, openat, unlinkat};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    for entry in Dir::read_from(directory)? {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        *entries += 1;
        anyhow::ensure!(
            *entries <= 65_536,
            "tenant owned cleanup entry limit exceeded"
        );
        let name = OsStr::from_bytes(bytes);
        match openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(child) => {
                clear_opened_directory(&child, entries)?;
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

#[cfg(windows)]
pub(super) fn clear_owned_directory_contents(_directory: &File, path: &Path) -> anyhow::Result<()> {
    clear_owned_directory_path(path, &mut 0_usize)
}

#[cfg(windows)]
fn clear_owned_directory_path(path: &Path, entries: &mut usize) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        *entries += 1;
        anyhow::ensure!(
            *entries <= 65_536,
            "tenant owned cleanup entry limit exceeded"
        );
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            clear_owned_directory_path(&entry.path(), entries)?;
            std::fs::remove_dir(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
pub(super) fn clear_owned_directory_contents(
    _directory: &File,
    _path: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("secure owned directory cleanup is unsupported on this platform")
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

#[cfg(test)]
pub(super) fn replace_temporary_source_for_test(
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

#[cfg(test)]
pub(super) fn write_incomplete_object_binding_for_test(
    _file: &File,
    path: &Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    #[cfg(unix)]
    rustix::fs::fsetxattr(
        _file,
        object_binding_name(),
        bytes,
        rustix::fs::XattrFlags::CREATE,
    )?;
    #[cfg(windows)]
    {
        let mut stream = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(object_binding_stream(path))?;
        stream.write_all(bytes)?;
        stream.sync_all()?;
    }
    let _ = path;
    Ok(())
}
