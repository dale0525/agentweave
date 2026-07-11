#[cfg(any(test, windows))]
pub(crate) const MOVEFILE_REPLACE_EXISTING_FLAG: u32 = 0x1;
#[cfg(any(test, windows))]
pub(crate) const MOVEFILE_WRITE_THROUGH_FLAG: u32 = 0x8;

#[cfg(any(test, windows))]
pub(crate) const fn atomic_replace_flags() -> u32 {
    MOVEFILE_REPLACE_EXISTING_FLAG | MOVEFILE_WRITE_THROUGH_FLAG
}

#[cfg(any(test, windows))]
pub(crate) fn normalized_path_is_within(path: &str, root: &str) -> bool {
    let path = path
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    let root = root
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

#[cfg(windows)]
mod platform {
    use super::{atomic_replace_flags, normalized_path_is_within};
    use anyhow::Context;
    use std::ffi::OsStr;
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Component, Path, PathBuf};
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetFileInformationByHandle, GetFinalPathNameByHandleW, MoveFileExW,
        OPEN_ALWAYS, OPEN_EXISTING,
    };

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub(crate) struct WindowsFileIdentity {
        volume_serial: u32,
        file_index: u64,
    }

    pub(crate) fn open_directory_nofollow(
        path: &Path,
    ) -> anyhow::Result<(File, WindowsFileIdentity, PathBuf)> {
        if !path.is_absolute() {
            anyhow::bail!("Windows store root must be absolute: {}", path.display());
        }
        let mut current = PathBuf::new();
        let mut opened = None;
        for component in path.components() {
            current.push(component.as_os_str());
            if !matches!(component, Component::Normal(_)) {
                continue;
            }
            let file = open_path(
                &current,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )?;
            let information = file_information(&file)?;
            reject_reparse_or_wrong_kind(&information, true, &current)?;
            opened = Some(file);
        }
        let file = opened.with_context(|| {
            format!(
                "Windows store root has no directory component: {}",
                path.display()
            )
        })?;
        let identity = file_identity(&file)?;
        let final_path = final_path(&file)?;
        Ok((file, identity, final_path))
    }

    pub(crate) fn verify_directory_path(
        path: &Path,
        expected: WindowsFileIdentity,
    ) -> anyhow::Result<()> {
        let (_, actual, _) = open_directory_nofollow(path)?;
        if actual != expected {
            anyhow::bail!("Windows store root identity changed: {}", path.display());
        }
        Ok(())
    }

    pub(crate) fn open_directory_beneath(
        root: &File,
        root_identity: WindowsFileIdentity,
        relative: &Path,
    ) -> anyhow::Result<(File, WindowsFileIdentity, PathBuf)> {
        let current_identity = file_identity(root)?;
        if current_identity != root_identity {
            anyhow::bail!("captured Windows store root handle identity changed");
        }
        let root_final = final_path(root)?;
        let mut current = root_final.clone();
        let mut opened = root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                anyhow::bail!("invalid Windows store-relative component");
            };
            current.push(name);
            let candidate = open_path(
                &current,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )?;
            let information = file_information(&candidate)?;
            reject_reparse_or_wrong_kind(&information, true, &current)?;
            let candidate_final = final_path(&candidate)?;
            ensure_contained(&candidate_final, &root_final)?;
            let identity = identity_from_information(&information);
            if identity.volume_serial != root_identity.volume_serial {
                anyhow::bail!("Windows store path crossed a volume boundary");
            }
            opened = candidate;
        }
        let identity = file_identity(&opened)?;
        let opened_final = final_path(&opened)?;
        ensure_contained(&opened_final, &root_final)?;
        Ok((opened, identity, opened_final))
    }

    pub(crate) fn open_lock_file_beneath(
        locks: &File,
        locks_identity: WindowsFileIdentity,
        file_name: &OsStr,
    ) -> anyhow::Result<File> {
        let current_identity = file_identity(locks)?;
        if current_identity != locks_identity {
            anyhow::bail!("captured Windows locks handle identity changed");
        }
        let locks_final = final_path(locks)?;
        let path = locks_final.join(file_name);
        let file = open_path(
            &path,
            GENERIC_READ | GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
        )?;
        let information = file_information(&file)?;
        reject_reparse_or_wrong_kind(&information, false, &path)?;
        let identity = identity_from_information(&information);
        if identity.volume_serial != locks_identity.volume_serial {
            anyhow::bail!("Windows revision lock crossed a volume boundary");
        }
        let opened_final = final_path(&file)?;
        ensure_contained(&opened_final, &locks_final)?;
        let parent = opened_final
            .parent()
            .context("Windows revision lock has no parent")?;
        if !paths_equal(parent, &locks_final) {
            anyhow::bail!("Windows revision lock escaped the captured locks directory");
        }
        Ok(file)
    }

    pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
        let source = wide_null(source.as_os_str());
        let destination = wide_null(destination.as_os_str());
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                atomic_replace_flags(),
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(crate) fn validate_tree_no_reparse(root: &Path) -> anyhow::Result<()> {
        let (_, root_identity, root_final) = open_directory_nofollow(root)?;
        let mut stack = vec![root_final.clone()];
        while let Some(directory) = stack.pop() {
            for entry in std::fs::read_dir(&directory)? {
                let path = entry?.path();
                let metadata = std::fs::symlink_metadata(&path)?;
                let flags = if metadata.is_dir() {
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
                } else {
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT
                };
                let file = open_path(&path, FILE_READ_ATTRIBUTES, OPEN_EXISTING, flags)?;
                let information = file_information(&file)?;
                reject_reparse_or_wrong_kind(&information, metadata.is_dir(), &path)?;
                let opened_final = final_path(&file)?;
                ensure_contained(&opened_final, &root_final)?;
                let identity = identity_from_information(&information);
                if identity.volume_serial != root_identity.volume_serial {
                    anyhow::bail!("Windows package tree crossed a volume boundary");
                }
                if metadata.is_dir() {
                    stack.push(opened_final);
                }
            }
        }
        Ok(())
    }

    fn open_path(path: &Path, access: u32, disposition: u32, flags: u32) -> anyhow::Result<File> {
        let wide = wide_null(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                disposition,
                flags,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!("failed to open Windows path safely: {}", path.display())
            });
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    }

    fn file_information(file: &File) -> anyhow::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        let result =
            unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut information) };
        if result == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(information)
        }
    }

    fn file_identity(file: &File) -> anyhow::Result<WindowsFileIdentity> {
        file_information(file).map(|information| identity_from_information(&information))
    }

    fn identity_from_information(information: &BY_HANDLE_FILE_INFORMATION) -> WindowsFileIdentity {
        WindowsFileIdentity {
            volume_serial: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        }
    }

    fn reject_reparse_or_wrong_kind(
        information: &BY_HANDLE_FILE_INFORMATION,
        directory: bool,
        path: &Path,
    ) -> anyhow::Result<()> {
        if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            anyhow::bail!(
                "Windows store path contains a reparse point: {}",
                path.display()
            );
        }
        let is_directory = information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if is_directory != directory {
            anyhow::bail!(
                "Windows store path has the wrong file type: {}",
                path.display()
            );
        }
        Ok(())
    }

    pub(crate) fn final_path_for_file(file: &File) -> anyhow::Result<PathBuf> {
        let handle = file.as_raw_handle() as HANDLE;
        let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
        if required == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut buffer = vec![0_u16; usize::try_from(required)? + 1];
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        };
        if written == 0 || usize::try_from(written)? >= buffer.len() {
            return Err(std::io::Error::last_os_error().into());
        }
        buffer.truncate(usize::try_from(written)?);
        Ok(PathBuf::from(String::from_utf16(&buffer)?))
    }

    fn final_path(file: &File) -> anyhow::Result<PathBuf> {
        final_path_for_file(file)
    }

    fn ensure_contained(path: &Path, root: &Path) -> anyhow::Result<()> {
        let path = path.to_string_lossy();
        let root = root.to_string_lossy();
        if !normalized_path_is_within(&path, &root) {
            anyhow::bail!("Windows store handle escaped its captured root: {path}");
        }
        Ok(())
    }

    fn paths_equal(left: &Path, right: &Path) -> bool {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }

    fn wide_null(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn crate_signatures_match_windows_sys_061() {
            let _create: unsafe extern "system" fn(
                *const u16,
                u32,
                u32,
                *const windows_sys::Win32::Security::SECURITY_ATTRIBUTES,
                u32,
                u32,
                HANDLE,
            ) -> HANDLE = CreateFileW;
            let _move: unsafe extern "system" fn(*const u16, *const u16, u32) -> i32 = MoveFileExW;
            let _final_path: unsafe extern "system" fn(HANDLE, *mut u16, u32, u32) -> u32 =
                GetFinalPathNameByHandleW;
        }
    }
}

#[cfg(windows)]
pub(crate) use platform::*;
