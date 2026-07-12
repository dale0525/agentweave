#[cfg(any(test, windows))]
pub(crate) const MOVEFILE_REPLACE_EXISTING_FLAG: u32 = 0x1;
#[cfg(any(test, windows))]
pub(crate) const MOVEFILE_WRITE_THROUGH_FLAG: u32 = 0x8;
#[cfg(any(test, windows))]
pub(crate) const FILE_SHARE_READ_FLAG: u32 = 0x1;
#[cfg(any(test, windows))]
pub(crate) const FILE_SHARE_WRITE_FLAG: u32 = 0x2;
#[cfg(any(test, windows))]
pub(crate) const FILE_SHARE_DELETE_FLAG: u32 = 0x4;
#[cfg(any(test, windows))]
pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT_FLAG: u32 = 0x400;
#[cfg(any(test, windows))]
pub(crate) const FILE_FLAG_BACKUP_SEMANTICS_FLAG: u32 = 0x02000000;
#[cfg(any(test, windows))]
pub(crate) const FILE_FLAG_OPEN_REPARSE_POINT_FLAG: u32 = 0x00200000;

#[cfg(any(test, windows))]
pub(crate) const fn atomic_replace_flags() -> u32 {
    MOVEFILE_REPLACE_EXISTING_FLAG | MOVEFILE_WRITE_THROUGH_FLAG
}

#[cfg(any(test, windows))]
pub(crate) const fn directory_share_mode() -> u32 {
    FILE_SHARE_READ_FLAG | FILE_SHARE_WRITE_FLAG
}

#[cfg(any(test, windows))]
pub(crate) const fn lock_file_share_mode() -> u32 {
    FILE_SHARE_READ_FLAG | FILE_SHARE_WRITE_FLAG
}

#[cfg(any(test, windows))]
pub(crate) const fn replaceable_file_share_mode() -> u32 {
    FILE_SHARE_READ_FLAG | FILE_SHARE_WRITE_FLAG | FILE_SHARE_DELETE_FLAG
}

#[cfg(any(test, windows))]
pub(crate) const fn regular_file_link_count_is_valid(link_count: u32) -> bool {
    link_count == 1
}

#[cfg(any(test, windows))]
pub(crate) const fn component_open_flags(directory: bool) -> u32 {
    FILE_FLAG_OPEN_REPARSE_POINT_FLAG
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS_FLAG
        } else {
            0
        }
}

#[cfg(any(test, windows))]
pub(crate) const fn attributes_are_reparse(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT_FLAG != 0
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

#[cfg(any(test, windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectoryBootstrapComponent {
    Prefix,
    Root,
    Normal,
}

#[cfg(any(test, windows))]
#[derive(Default)]
pub(crate) struct DirectoryBootstrapState {
    has_prefix: bool,
    has_root: bool,
}

#[cfg(any(test, windows))]
impl DirectoryBootstrapState {
    pub(crate) fn should_open(&mut self, component: DirectoryBootstrapComponent) -> bool {
        match component {
            DirectoryBootstrapComponent::Prefix => {
                self.has_prefix = true;
                false
            }
            DirectoryBootstrapComponent::Root => {
                self.has_root = self.has_prefix;
                self.has_root
            }
            DirectoryBootstrapComponent::Normal => self.has_root,
        }
    }
}

#[cfg(any(test, windows))]
pub(crate) fn finish_directory_child_creation<T, F>(
    create: std::io::Result<()>,
    open: F,
) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    match create {
        Ok(()) => open(),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => open(),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
mod platform {
    use super::{
        DirectoryBootstrapComponent, DirectoryBootstrapState, atomic_replace_flags,
        attributes_are_reparse, component_open_flags, directory_share_mode, lock_file_share_mode,
        normalized_path_is_within, regular_file_link_count_is_valid, replaceable_file_share_mode,
    };
    use anyhow::Context;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Component, Path, PathBuf};
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_BASIC_INFO, FILE_DISPOSITION_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES, FileBasicInfo,
        FileDispositionInfo, GetFileInformationByHandle, GetFinalPathNameByHandleW, MoveFileExW,
        OPEN_ALWAYS, OPEN_EXISTING, SetFileInformationByHandle,
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
        let mut bootstrap = DirectoryBootstrapState::default();
        for component in path.components() {
            current.push(component.as_os_str());
            let bootstrap_component = match component {
                Component::Prefix(_) => DirectoryBootstrapComponent::Prefix,
                Component::RootDir => DirectoryBootstrapComponent::Root,
                Component::Normal(_) => DirectoryBootstrapComponent::Normal,
                Component::CurDir | Component::ParentDir => continue,
            };
            if !bootstrap.should_open(bootstrap_component) {
                continue;
            }
            let file = open_path(
                &current,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                component_open_flags(true),
                directory_share_mode(),
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

    pub(crate) fn prepare_directory_path_nofollow(path: &Path) -> anyhow::Result<()> {
        if !path.is_absolute() {
            anyhow::bail!("Windows store root must be absolute: {}", path.display());
        }
        let mut ancestor = path;
        let mut missing = Vec::new();
        let (mut directory, _, _) = loop {
            match open_directory_nofollow(ancestor) {
                Ok(opened) => break opened,
                Err(error) if error_is_not_found(&error) => {
                    missing.push(
                        ancestor
                            .file_name()
                            .context("Windows store path has no existing ancestor")?
                            .to_os_string(),
                    );
                    ancestor = ancestor
                        .parent()
                        .context("Windows store path has no existing ancestor")?;
                }
                Err(error) => return Err(error),
            }
        };
        for name in missing.into_iter().rev() {
            let parent_identity = file_identity(&directory)?;
            let (child, _, _, _) =
                create_or_open_directory_child(&directory, parent_identity, &name)?;
            directory = child;
        }
        Ok(())
    }

    pub(crate) fn create_or_open_directory_child(
        parent_handle: &File,
        parent_identity: WindowsFileIdentity,
        child_name: &OsStr,
    ) -> anyhow::Result<(File, WindowsFileIdentity, PathBuf, bool)> {
        let components = Path::new(child_name).components().collect::<Vec<_>>();
        if !matches!(components.as_slice(), [Component::Normal(_)]) {
            anyhow::bail!("invalid Windows direct-child directory name");
        }
        let parent = WindowsStableParent {
            handle: parent_handle.try_clone()?,
            final_path: final_path(parent_handle)?,
            identity: parent_identity,
        };
        let child_path = parent.final_path.join(child_name);
        let create = std::fs::create_dir(&child_path);
        let created = create.is_ok();
        let child = super::finish_directory_child_creation(create, || {
            open_direct_child(
                &parent,
                child_name,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                true,
            )
        })?;
        let identity = file_identity(&child)?;
        let child_final = final_path(&child)?;
        Ok((child, identity, child_final, created))
    }

    pub(crate) fn identity_for_file(file: &File) -> anyhow::Result<WindowsFileIdentity> {
        file_identity(file)
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
                component_open_flags(true),
                directory_share_mode(),
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

    pub(crate) fn open_mutable_directory_beneath(
        root: &File,
        root_identity: WindowsFileIdentity,
        relative: &Path,
    ) -> anyhow::Result<(File, WindowsFileIdentity, PathBuf)> {
        if file_identity(root)? != root_identity {
            anyhow::bail!("captured Windows store root handle identity changed");
        }
        let (parent, name) = open_stable_parent(root, relative)?;
        let directory = open_direct_child(
            &parent,
            &name,
            DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
            OPEN_EXISTING,
            true,
        )?;
        let identity = file_identity(&directory)?;
        let final_path = final_path(&directory)?;
        Ok((directory, identity, final_path))
    }

    pub(crate) fn open_verification_directory_beneath(
        root: &File,
        root_identity: WindowsFileIdentity,
        relative: &Path,
    ) -> anyhow::Result<File> {
        if file_identity(root)? != root_identity {
            anyhow::bail!("captured Windows store root handle identity changed");
        }
        let (parent, name) = open_stable_parent(root, relative)?;
        open_direct_child_with_share(
            &parent,
            &name,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            OPEN_EXISTING,
            true,
            directory_share_mode() | super::FILE_SHARE_DELETE_FLAG,
        )
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
            FILE_ATTRIBUTE_NORMAL | component_open_flags(false),
            lock_file_share_mode(),
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

    pub(crate) struct WindowsStableParent {
        handle: File,
        final_path: PathBuf,
        identity: WindowsFileIdentity,
    }

    impl WindowsStableParent {
        pub(crate) fn child_path(&self, name: &OsStr) -> PathBuf {
            self.final_path.join(name)
        }

        pub(crate) fn create_new_regular(&self, name: &OsStr) -> anyhow::Result<File> {
            open_direct_child(
                self,
                name,
                GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                CREATE_NEW,
                false,
            )
        }

        pub(crate) fn create_new_replaceable_regular(&self, name: &OsStr) -> anyhow::Result<File> {
            open_direct_child_with_share(
                self,
                name,
                GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                CREATE_NEW,
                false,
                replaceable_file_share_mode(),
            )
        }

        pub(crate) fn atomic_replace(
            &self,
            source_name: &OsStr,
            destination_name: &OsStr,
            replaceable_destination: bool,
        ) -> anyhow::Result<()> {
            atomic_replace(
                &self.final_path.join(source_name),
                &self.final_path.join(destination_name),
            )?;
            let share_mode = if replaceable_destination {
                replaceable_file_share_mode()
            } else {
                directory_share_mode()
            };
            let destination = open_direct_child_with_share(
                self,
                destination_name,
                FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                false,
                share_mode,
            )?;
            drop(destination);
            Ok(())
        }

        pub(crate) fn remove_regular(&self, name: &OsStr) -> anyhow::Result<()> {
            let file = open_direct_child(
                self,
                name,
                DELETE | FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
                OPEN_EXISTING,
                false,
            )?;
            set_file_readonly_handle(&file, false)?;
            set_delete_disposition(&file)
        }
    }

    pub(crate) fn open_stable_parent(
        root: &File,
        relative: &Path,
    ) -> anyhow::Result<(WindowsStableParent, OsString)> {
        let name = relative
            .file_name()
            .context("Windows relative file path has no name")?
            .to_os_string();
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let root_identity = file_identity(root)?;
        let (handle, identity, final_path) = open_directory_beneath(root, root_identity, parent)?;
        Ok((
            WindowsStableParent {
                handle,
                final_path,
                identity,
            },
            name,
        ))
    }

    pub(crate) fn open_regular_file_beneath(
        root: &File,
        relative: &Path,
        writable: bool,
        create_new: bool,
    ) -> anyhow::Result<(File, u64)> {
        open_regular_file_beneath_with_share(root, relative, writable, create_new, false)
    }

    pub(crate) fn open_replaceable_regular_file_beneath(
        root: &File,
        relative: &Path,
    ) -> anyhow::Result<(File, u64)> {
        open_regular_file_beneath_with_share(root, relative, false, false, true)
    }

    fn open_regular_file_beneath_with_share(
        root: &File,
        relative: &Path,
        writable: bool,
        create_new: bool,
        replaceable: bool,
    ) -> anyhow::Result<(File, u64)> {
        let (parent, name) = open_stable_parent(root, relative)?;
        let access = if writable {
            GENERIC_WRITE | FILE_READ_ATTRIBUTES
        } else {
            GENERIC_READ | FILE_READ_ATTRIBUTES
        };
        let disposition = if create_new {
            CREATE_NEW
        } else {
            OPEN_EXISTING
        };
        let share_mode = if replaceable {
            replaceable_file_share_mode()
        } else {
            directory_share_mode()
        };
        let file =
            open_direct_child_with_share(&parent, &name, access, disposition, false, share_mode)?;
        let information = file_information(&file)?;
        if !regular_file_link_count_is_valid(information.nNumberOfLinks) {
            anyhow::bail!(
                "prepared package source cannot contain hard links: {}",
                parent.child_path(&name).display()
            );
        }
        let length =
            (u64::from(information.nFileSizeHigh) << 32) | u64::from(information.nFileSizeLow);
        Ok((file, length))
    }

    pub(crate) struct WindowsOpenedEntry {
        pub(crate) file: File,
        pub(crate) is_directory: bool,
        pub(crate) length: u64,
        pub(crate) link_count: u32,
        pub(crate) final_path: PathBuf,
    }

    pub(crate) fn open_child_entry(
        parent: &File,
        name: &OsStr,
    ) -> anyhow::Result<WindowsOpenedEntry> {
        let parent_identity = file_identity(parent)?;
        let parent_final = final_path(parent)?;
        let path = parent_final.join(name);
        let file = open_path(
            &path,
            GENERIC_READ | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            OPEN_EXISTING,
            component_open_flags(true),
            directory_share_mode(),
        )?;
        let information = file_information(&file)?;
        if attributes_are_reparse(information.dwFileAttributes) {
            anyhow::bail!(
                "Windows store path contains a reparse point: {}",
                path.display()
            );
        }
        let identity = identity_from_information(&information);
        if identity.volume_serial != parent_identity.volume_serial {
            anyhow::bail!("Windows child crossed its opened parent volume");
        }
        let opened = final_path(&file)?;
        if !paths_equal(
            opened
                .parent()
                .context("opened Windows child has no parent")?,
            &parent_final,
        ) {
            anyhow::bail!("opened Windows child escaped its stable parent");
        }
        Ok(WindowsOpenedEntry {
            file,
            is_directory: information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            length: (u64::from(information.nFileSizeHigh) << 32)
                | u64::from(information.nFileSizeLow),
            link_count: information.nNumberOfLinks,
            final_path: opened,
        })
    }

    pub(crate) fn validate_target_beneath(
        root: &File,
        relative: Option<&Path>,
        directory: bool,
    ) -> anyhow::Result<()> {
        match relative {
            None => {
                let information = file_information(root)?;
                reject_reparse_or_wrong_kind(&information, directory, Path::new("<opened-root>"))
            }
            Some(relative) if directory => {
                let identity = file_identity(root)?;
                open_directory_beneath(root, identity, relative).map(|_| ())
            }
            Some(relative) => open_regular_file_beneath(root, relative, false, false).map(|_| ()),
        }
    }

    pub(crate) fn set_readonly_beneath(
        root: &File,
        relative: Option<&Path>,
        directory: bool,
        readonly: bool,
    ) -> anyhow::Result<()> {
        let file = match relative {
            None => root.try_clone()?,
            Some(relative) if directory => {
                let (parent, name) = open_stable_parent(root, relative)?;
                open_direct_child(
                    &parent,
                    &name,
                    FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
                    OPEN_EXISTING,
                    true,
                )?
            }
            Some(relative) => {
                let (parent, name) = open_stable_parent(root, relative)?;
                open_direct_child(
                    &parent,
                    &name,
                    FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
                    OPEN_EXISTING,
                    false,
                )?
            }
        };
        set_file_readonly_handle(&file, readonly)
    }

    pub(crate) fn delete_opened_tree(directory: &File) -> anyhow::Result<()> {
        delete_opened_directory(directory)
    }

    pub(crate) fn delete_opened_empty_directory(directory: &File) -> anyhow::Result<()> {
        set_file_readonly_handle(directory, false)?;
        set_delete_disposition(directory)
    }

    fn delete_opened_directory(directory: &File) -> anyhow::Result<()> {
        let path = final_path(&directory)?;
        let names = std::fs::read_dir(&path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        let parent = WindowsStableParent {
            identity: file_identity(&directory)?,
            final_path: path,
            handle: directory.try_clone()?,
        };
        for name in names {
            let entry = open_direct_child(
                &parent,
                &name,
                DELETE
                    | GENERIC_READ
                    | FILE_LIST_DIRECTORY
                    | FILE_READ_ATTRIBUTES
                    | FILE_WRITE_ATTRIBUTES,
                OPEN_EXISTING,
                true,
            )
            .or_else(|directory_error| {
                open_direct_child(
                    &parent,
                    &name,
                    DELETE | GENERIC_READ | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
                    OPEN_EXISTING,
                    false,
                )
                .map_err(|file_error| {
                    anyhow::anyhow!(
                        "failed to open Windows delete entry as directory ({directory_error:#}) or file ({file_error:#})"
                    )
                })
            })?;
            let information = file_information(&entry)?;
            if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                delete_opened_directory(&entry)?;
            } else {
                set_file_readonly_handle(&entry, false)?;
                set_delete_disposition(&entry)?;
            }
        }
        drop(parent);
        set_file_readonly_handle(&directory, false)?;
        set_delete_disposition(&directory)
    }

    fn open_direct_child(
        parent: &WindowsStableParent,
        name: &OsStr,
        access: u32,
        disposition: u32,
        directory: bool,
    ) -> anyhow::Result<File> {
        open_direct_child_with_share(
            parent,
            name,
            access,
            disposition,
            directory,
            directory_share_mode(),
        )
    }

    fn open_direct_child_with_share(
        parent: &WindowsStableParent,
        name: &OsStr,
        access: u32,
        disposition: u32,
        directory: bool,
        share_mode: u32,
    ) -> anyhow::Result<File> {
        if file_identity(&parent.handle)? != parent.identity {
            anyhow::bail!("opened Windows parent identity changed");
        }
        let path = parent.final_path.join(name);
        let flags = if directory {
            component_open_flags(true)
        } else {
            FILE_ATTRIBUTE_NORMAL | component_open_flags(false)
        };
        let file = open_path(&path, access, disposition, flags, share_mode)?;
        let information = file_information(&file)?;
        reject_reparse_or_wrong_kind(&information, directory, &path)?;
        let identity = identity_from_information(&information);
        if identity.volume_serial != parent.identity.volume_serial {
            anyhow::bail!("Windows child crossed its opened parent volume");
        }
        let opened = final_path(&file)?;
        let opened_parent = opened
            .parent()
            .context("opened Windows child has no parent")?;
        if !paths_equal(opened_parent, &parent.final_path) {
            anyhow::bail!("opened Windows child escaped its stable parent");
        }
        Ok(file)
    }

    fn set_file_readonly_handle(file: &File, readonly: bool) -> anyhow::Result<()> {
        let current = file_information(file)?;
        let attributes = if readonly {
            current.dwFileAttributes | FILE_ATTRIBUTE_READONLY
        } else {
            current.dwFileAttributes & !FILE_ATTRIBUTE_READONLY
        };
        let information = FILE_BASIC_INFO {
            FileAttributes: attributes,
            ..FILE_BASIC_INFO::default()
        };
        let result = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileBasicInfo,
                std::ptr::from_ref(&information).cast(),
                u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>())?,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    fn set_delete_disposition(file: &File) -> anyhow::Result<()> {
        let information = FILE_DISPOSITION_INFO { DeleteFile: true };
        let result = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                std::ptr::from_ref(&information).cast(),
                u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>())?,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    fn error_is_not_found(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        })
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

    fn open_path(
        path: &Path,
        access: u32,
        disposition: u32,
        flags: u32,
        share_mode: u32,
    ) -> anyhow::Result<File> {
        let wide = wide_null(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                share_mode,
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
        if attributes_are_reparse(information.dwFileAttributes) {
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
