#[cfg(any(test, windows))]
pub(crate) const FILE_CREATE_DISPOSITION: u32 = 2;
#[cfg(any(test, windows))]
pub(crate) const FILE_DIRECTORY_CREATE_OPTION: u32 = 1;
#[cfg(any(test, windows))]
pub(crate) const FILE_OPEN_REPARSE_CREATE_OPTION: u32 = 0x0020_0000;
#[cfg(any(test, windows))]
pub(crate) const FILE_SYNCHRONOUS_CREATE_OPTION: u32 = 0x20;

#[cfg(any(test, windows))]
#[derive(Debug)]
pub(crate) enum NativeDirectoryCreate<T> {
    Created(T),
    AlreadyExists,
}

#[cfg(windows)]
pub(crate) fn create_directory_child_atomically(
    parent: &std::fs::File,
    child_name: &std::ffi::OsStr,
    access: u32,
    share_mode: u32,
) -> std::io::Result<NativeDirectoryCreate<std::fs::File>> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::NtCreateFile;
    use windows_sys::Win32::Foundation::{
        HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE, STATUS_OBJECT_NAME_COLLISION,
        UNICODE_STRING,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SYNCHRONIZE};
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut name = child_name.encode_wide().collect::<Vec<_>>();
    if name.is_empty() || name.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Windows direct-child directory name",
        ));
    }
    let byte_length = u16::try_from(name.len().saturating_mul(2)).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Windows direct-child directory name is too long",
        )
    })?;
    let unicode_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).unwrap(),
        RootDirectory: parent.as_raw_handle() as HANDLE,
        ObjectName: std::ptr::from_ref(&unicode_name),
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut status_block = IO_STATUS_BLOCK::default();
    let mut handle = INVALID_HANDLE_VALUE;
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut status_block,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share_mode,
            FILE_CREATE_DISPOSITION,
            FILE_DIRECTORY_CREATE_OPTION
                | FILE_OPEN_REPARSE_CREATE_OPTION
                | FILE_SYNCHRONOUS_CREATE_OPTION,
            std::ptr::null(),
            0,
        )
    };
    if status == STATUS_OBJECT_NAME_COLLISION {
        return Ok(NativeDirectoryCreate::AlreadyExists);
    }
    if status < 0 {
        let code = unsafe { windows_sys::Win32::Foundation::RtlNtStatusToDosError(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32));
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::other(
            "NtCreateFile succeeded without a directory handle",
        ));
    }
    Ok(NativeDirectoryCreate::Created(unsafe {
        std::fs::File::from_raw_handle(handle)
    }))
}

#[cfg(all(test, windows))]
#[test]
fn nt_create_file_signature_matches_windows_sys_061() {
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Win32::Foundation::{HANDLE, NTSTATUS};
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let _native: unsafe extern "system" fn(
        *mut HANDLE,
        u32,
        *const OBJECT_ATTRIBUTES,
        *mut IO_STATUS_BLOCK,
        *const i64,
        u32,
        u32,
        u32,
        u32,
        *const core::ffi::c_void,
        u32,
    ) -> NTSTATUS = windows_sys::Wdk::Storage::FileSystem::NtCreateFile;
}

#[cfg(all(test, windows))]
#[test]
fn native_create_returns_the_created_directory_handle_and_reports_existing_separately() {
    let temp = tempfile::tempdir().unwrap();
    let (parent, parent_identity, _) =
        crate::skill_store_windows::open_directory_nofollow(temp.path()).unwrap();
    let name = std::ffi::OsStr::new("atomic-child");
    let created = create_directory_child_atomically(
        &parent,
        name,
        crate::skill_store_windows::bootstrap_directory_access_mask(),
        crate::skill_store_windows::directory_share_mode(),
    )
    .unwrap();
    let NativeDirectoryCreate::Created(handle) = created else {
        panic!("first native directory create did not return Created(handle)");
    };
    let created_identity = crate::skill_store_windows::identity_for_file(&handle).unwrap();
    assert_ne!(created_identity, parent_identity);

    let existing = create_directory_child_atomically(
        &parent,
        name,
        crate::skill_store_windows::bootstrap_directory_access_mask(),
        crate::skill_store_windows::directory_share_mode(),
    )
    .unwrap();
    assert!(matches!(existing, NativeDirectoryCreate::AlreadyExists));
    drop(handle);
    std::fs::remove_dir(temp.path().join(name)).unwrap();
}
