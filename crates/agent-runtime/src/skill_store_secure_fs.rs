use crate::skill_package::{LoadedPackageDescriptor, SkillPackageDescriptor};
use crate::skill_source::{
    canonical_relative_path, portable_collision_key, register_portable_path,
};
use crate::skill_store_fs::PackageLimits;
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::sync::{Arc, Mutex, OnceLock};

const TREE_HASH_DOMAIN: &[u8] = b"general-agent.skill-package-tree";
const TREE_HASH_VERSION: u32 = 1;
const TREE_HASH_FILE_ENTRY: u8 = 1;
const READ_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct SecureHashTestGate {
    entered: Arc<std::sync::Barrier>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(test)]
impl SecureHashTestGate {
    pub(crate) async fn wait_entered(&self) {
        let barrier = self.entered.clone();
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
    }

    pub(crate) async fn release(&self) {
        let barrier = self.release.clone();
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
    }
}

#[cfg(test)]
pub(crate) fn gate_secure_hash_after_open() -> SecureHashTestGate {
    let gate = SecureHashTestGate {
        entered: Arc::new(std::sync::Barrier::new(2)),
        release: Arc::new(std::sync::Barrier::new(2)),
    };
    *secure_hash_gate().lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn secure_hash_gate() -> &'static Mutex<Option<SecureHashTestGate>> {
    static GATE: OnceLock<Mutex<Option<SecureHashTestGate>>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn checkpoint_secure_hash_after_open() {
    if let Some(gate) = secure_hash_gate().lock().unwrap().take() {
        gate.entered.wait();
        gate.release.wait();
    }
}

#[cfg(not(test))]
fn checkpoint_secure_hash_after_open() {}

pub(crate) struct SecurePackageSnapshot {
    pub descriptor: LoadedPackageDescriptor,
    pub content_hash: String,
    pub runtime_manifest: Option<Vec<u8>>,
    pub instructions_file: Option<Vec<u8>>,
}

#[cfg(test)]
pub(crate) use crate::skill_store_secure_fs_faults::inject_transient_directory_open_once;

pub(crate) struct SecureTreeSnapshot {
    pub content_hash: String,
    descriptor_bytes: Option<Vec<u8>>,
    runtime_manifest: Option<Vec<u8>>,
    instructions_file: Option<Vec<u8>>,
}

impl SecureTreeSnapshot {
    pub(crate) fn load_descriptor(&self, root: &Path) -> anyhow::Result<LoadedPackageDescriptor> {
        SkillPackageDescriptor::load_from_file_bytes(
            root,
            self.descriptor_bytes.clone(),
            self.runtime_manifest.clone(),
            self.instructions_file.clone(),
        )
    }
}

pub(crate) fn unbounded_package_limits() -> PackageLimits {
    PackageLimits {
        max_file_bytes: u64::MAX,
        max_package_bytes: u64::MAX,
        max_entries: u64::MAX,
        max_files: u64::MAX,
        max_directories: u64::MAX,
        max_depth: u64::MAX,
        max_relative_path_bytes: u64::MAX,
    }
}

pub(crate) async fn secure_package_snapshot(
    root: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || snapshot_direct(&root, limits))
        .await
        .context("secure package snapshot worker failed")?
}

pub(crate) async fn secure_package_hash(
    root: &Path,
    limits: PackageLimits,
) -> anyhow::Result<String> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || hash_direct(&root, limits))
        .await
        .context("secure package hash worker failed")?
}

pub(crate) async fn secure_tree_snapshot(
    root: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || tree_direct(&root, limits))
        .await
        .context("secure tree snapshot worker failed")?
}

pub(crate) async fn secure_package_snapshot_beneath(
    trusted_root: &Path,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    canonical_relative_path(relative)?;
    let trusted_root = trusted_root.to_path_buf();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || snapshot_beneath(&trusted_root, &relative, limits))
        .await
        .context("secure package snapshot worker failed")?
}

pub(crate) async fn ensure_store_directory(
    trusted_root: &Path,
    relative: &Path,
) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let trusted_root = trusted_root.to_path_buf();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || ensure_directory_beneath(&trusted_root, &relative))
        .await
        .context("secure directory preparation worker failed")?
}

pub(crate) async fn prepare_directory_path(path: &Path) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || prepare_directory_path_platform(&path))
        .await
        .context("secure directory path preparation worker failed")?
}

pub(crate) async fn remove_store_tree(trusted_root: &Path, relative: &Path) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let trusted_root = trusted_root.to_path_buf();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || remove_tree_beneath(&trusted_root, &relative))
        .await
        .context("secure tree cleanup worker failed")?
}

pub(crate) async fn reserve_store_directory(
    trusted_root: &Path,
    relative: &Path,
) -> anyhow::Result<()> {
    canonical_relative_path(relative)?;
    let trusted_root = trusted_root.to_path_buf();
    let relative = relative.to_path_buf();
    tokio::task::spawn_blocking(move || reserve_directory_beneath(&trusted_root, &relative))
        .await
        .context("secure directory reservation worker failed")?
}

#[cfg(unix)]
fn open_trusted_directory(root: &Path) -> anyhow::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, open};
    open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| format!("failed to open trusted store root: {}", root.display()))
}

#[cfg(unix)]
fn prepare_directory_path_platform(path: &Path) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, open, openat};
    if !path.is_absolute() {
        anyhow::bail!("skill store root must be absolute: {}", path.display());
    }
    let mut directory = open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    for component in path.components() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
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
        )
        .with_context(|| {
            format!(
                "failed to prepare directory path without following symlinks: {}",
                path.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn remove_tree_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};
    let mut parent = open_trusted_directory(root)?;
    let components = relative.components().collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .context("store cleanup path is empty")?;
    for component in parents {
        parent = openat(
            &parent,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    let target = match openat(
        &parent,
        name.as_os_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(target) => target,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    remove_open_directory_contents(&target)?;
    match unlinkat(&parent, name.as_os_str(), AtFlags::REMOVEDIR) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn remove_open_directory_contents(directory: &std::os::fd::OwnedFd) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Dir, Mode, OFlags, openat, unlinkat};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let entries = Dir::read_from(directory)?;
    for entry in entries {
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
                remove_open_directory_contents(&child)?;
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
fn ensure_directory_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut directory = open_trusted_directory(root)?;
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
        )
        .with_context(|| {
            format!(
                "failed to open prepared store directory without following symlinks: {}",
                root.join(relative).display()
            )
        })?;
    }
    Ok(())
}

#[cfg(unix)]
fn reserve_directory_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    use rustix::fs::{Mode, OFlags, RawMode, mkdirat, openat};
    let mut parent = open_trusted_directory(root)?;
    let components = relative.components().collect::<Vec<_>>();
    let (name, parents) = components
        .split_last()
        .context("store reservation path is empty")?;
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
            root.join(relative).display()
        ),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn ensure_directory_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(root.join(relative))?;
    let canonical_root = std::fs::canonicalize(root)?;
    let canonical = std::fs::canonicalize(root.join(relative))?;
    if !canonical.starts_with(canonical_root) {
        anyhow::bail!("prepared store directory escapes trusted root");
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_directory_path_platform(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    Ok(())
}

#[cfg(not(unix))]
fn remove_tree_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    let canonical_root = std::fs::canonicalize(root)?;
    let target = root.join(relative);
    match std::fs::canonicalize(&target) {
        Ok(canonical) if canonical.starts_with(canonical_root) => std::fs::remove_dir_all(target)?,
        Ok(_) => anyhow::bail!("cleanup target escapes trusted store root"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(not(unix))]
fn reserve_directory_beneath(root: &Path, relative: &Path) -> anyhow::Result<()> {
    std::fs::create_dir(root.join(relative))?;
    Ok(())
}

#[cfg(unix)]
fn snapshot_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<SecurePackageSnapshot> {
    use rustix::fs::{Mode, OFlags, open};
    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open package root without following symlinks: {}",
            root.display()
        )
    })?;
    snapshot_opened(root, root_fd, limits)
}

#[cfg(unix)]
fn hash_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<String> {
    use rustix::fs::{Mode, OFlags, open};
    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    Ok(scan_opened(root, root_fd, limits)?.content_hash)
}

#[cfg(unix)]
fn tree_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<SecureTreeSnapshot> {
    use rustix::fs::{Mode, OFlags, open};
    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    scan_opened(root, root_fd, limits)
}

#[cfg(unix)]
fn snapshot_beneath(
    trusted_root: &Path,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let mut directory = open(
        trusted_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open trusted store root: {}",
            trusted_root.display()
        )
    })?;
    for component in relative.components() {
        directory = openat(
            &directory,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .with_context(|| {
            format!(
                "failed to open managed package path without following symlinks: {}",
                trusted_root.join(relative).display()
            )
        })?;
    }
    snapshot_opened(&trusted_root.join(relative), directory, limits)
}

#[cfg(unix)]
struct PackageFileIdentity {
    relative: PathBuf,
    canonical: Vec<u8>,
    expected_bytes: u64,
}

#[cfg(unix)]
struct WalkState {
    limits: PackageLimits,
    entries: u64,
    files: u64,
    directories: u64,
    package_bytes: u64,
    portable_paths: BTreeMap<Vec<u8>, PathBuf>,
    files_to_hash: Vec<PackageFileIdentity>,
}

#[cfg(unix)]
fn snapshot_opened(
    display_root: &Path,
    root_fd: std::os::fd::OwnedFd,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let mut state = WalkState {
        limits,
        entries: 0,
        files: 0,
        directories: 0,
        package_bytes: 0,
        portable_paths: BTreeMap::new(),
        files_to_hash: Vec::new(),
    };
    walk_open_directory(&root_fd, Path::new(""), display_root, &mut state)?;
    state
        .files_to_hash
        .sort_by(|left, right| left.canonical.cmp(&right.canonical));
    let scanned = scan_relative_files(display_root, &root_fd, state.files_to_hash)?;
    let descriptor = SkillPackageDescriptor::load_from_file_bytes(
        display_root,
        scanned.descriptor_bytes.clone(),
        scanned.runtime_manifest.clone(),
        scanned.instructions_file.clone(),
    )?;
    Ok(SecurePackageSnapshot {
        descriptor,
        content_hash: scanned.content_hash,
        runtime_manifest: scanned.runtime_manifest,
        instructions_file: scanned.instructions_file,
    })
}

#[cfg(unix)]
fn scan_opened(
    display_root: &Path,
    root_fd: std::os::fd::OwnedFd,
    limits: PackageLimits,
) -> anyhow::Result<SecureTreeSnapshot> {
    let mut state = WalkState {
        limits,
        entries: 0,
        files: 0,
        directories: 0,
        package_bytes: 0,
        portable_paths: BTreeMap::new(),
        files_to_hash: Vec::new(),
    };
    walk_open_directory(&root_fd, Path::new(""), display_root, &mut state)?;
    state
        .files_to_hash
        .sort_by(|left, right| left.canonical.cmp(&right.canonical));
    checkpoint_secure_hash_after_open();
    scan_relative_files(display_root, &root_fd, state.files_to_hash)
}

#[cfg(unix)]
fn walk_open_directory(
    directory: &std::os::fd::OwnedFd,
    relative_directory: &Path,
    display_root: &Path,
    state: &mut WalkState,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let entries = Dir::read_from(directory)?;
    for entry in entries {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        let name = OsStr::from_bytes(bytes);
        let relative = relative_directory.join(name);
        validate_relative_entry(&relative, state)?;
        crate::skill_store_secure_fs_faults::check_directory_open(display_root)?;
        let directory_open = openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        );
        match directory_open {
            Ok(child) => {
                state.directories =
                    checked_count(state.directories, state.limits.max_directories, "directory")?;
                walk_open_directory(&child, &relative, display_root, state)?;
                continue;
            }
            Err(rustix::io::Errno::NOTDIR) | Err(rustix::io::Errno::LOOP) => {}
            Err(error) => return Err(error.into()),
        }
        let entry_stat = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let entry_type = FileType::from_raw_mode(entry_stat.st_mode);
        if entry_type == FileType::Symlink {
            anyhow::bail!(
                "skill package cannot contain symlinks: {}",
                display_root.join(&relative).display()
            );
        }
        if entry_type != FileType::RegularFile {
            anyhow::bail!(
                "skill package cannot contain special files: {}",
                display_root.join(&relative).display()
            );
        }
        let expected_bytes =
            u64::try_from(entry_stat.st_size).context("package file has negative size")?;
        if expected_bytes > state.limits.max_file_bytes {
            anyhow::bail!(
                "skill package file exceeds {} byte limit: {}",
                state.limits.max_file_bytes,
                display_root.join(&relative).display()
            );
        }
        state.package_bytes = checked_bytes(
            state.package_bytes,
            expected_bytes,
            state.limits.max_package_bytes,
        )?;
        state.files = checked_count(state.files, state.limits.max_files, "file")?;
        state.files_to_hash.push(PackageFileIdentity {
            canonical: canonical_relative_path(&relative)?,
            relative,
            expected_bytes,
        });
    }
    Ok(())
}

#[cfg(unix)]
fn validate_relative_entry(relative: &Path, state: &mut WalkState) -> anyhow::Result<()> {
    let canonical = canonical_relative_path(relative)?;
    if u64::try_from(canonical.len())? > state.limits.max_relative_path_bytes {
        anyhow::bail!(
            "skill package relative path exceeds {} byte limit: {}",
            state.limits.max_relative_path_bytes,
            relative.display()
        );
    }
    if u64::try_from(relative.components().count())? > state.limits.max_depth {
        anyhow::bail!(
            "skill package path depth exceeds {} component limit: {}",
            state.limits.max_depth,
            relative.display()
        );
    }
    state.entries = checked_count(state.entries, state.limits.max_entries, "entry")?;
    let collision_key = portable_collision_key(relative)?;
    register_portable_path(&mut state.portable_paths, relative, &collision_key)
}

#[cfg(unix)]
fn scan_relative_files(
    display_root: &Path,
    root_fd: &std::os::fd::OwnedFd,
    files: Vec<PackageFileIdentity>,
) -> anyhow::Result<SecureTreeSnapshot> {
    use std::fs::File;
    use std::io::Read;

    let mut hasher = Sha256::new();
    hasher.update(TREE_HASH_DOMAIN);
    hasher.update(TREE_HASH_VERSION.to_be_bytes());
    let mut descriptor_bytes = None;
    let mut runtime_manifest = None;
    let mut instructions_file = None;
    for opened in files {
        let descriptor = open_relative_file(root_fd, &opened.relative, display_root)?;
        let stat = rustix::fs::fstat(&descriptor)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
            anyhow::bail!(
                "skill package cannot contain special files: {}",
                display_root.join(&opened.relative).display()
            );
        }
        let opened_bytes = u64::try_from(stat.st_size).context("package file has negative size")?;
        if opened_bytes != opened.expected_bytes {
            anyhow::bail!(
                "package file changed before hashing: {}",
                display_root.join(&opened.relative).display()
            );
        }
        hasher.update([TREE_HASH_FILE_ENTRY]);
        hasher.update(u64::try_from(opened.canonical.len())?.to_be_bytes());
        hasher.update(&opened.canonical);
        hasher.update(opened.expected_bytes.to_be_bytes());
        let capture = matches!(
            opened.canonical.as_slice(),
            b"general-agent.json" | b"skill.json" | b"SKILL.md"
        );
        let mut captured = capture.then(Vec::new);
        let mut file = File::from(descriptor);
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        let mut bytes_read = 0_u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes_read = bytes_read
                .checked_add(u64::try_from(count)?)
                .context("package file length overflowed while hashing")?;
            hasher.update(&buffer[..count]);
            if let Some(bytes) = &mut captured {
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
        if bytes_read != opened.expected_bytes {
            anyhow::bail!(
                "package file changed while hashing: {}",
                display_root.join(&opened.relative).display()
            );
        }
        match opened.canonical.as_slice() {
            b"general-agent.json" => descriptor_bytes = captured,
            b"skill.json" => runtime_manifest = captured,
            b"SKILL.md" => instructions_file = captured,
            _ => {}
        }
    }
    Ok(SecureTreeSnapshot {
        content_hash: hex::encode(hasher.finalize()),
        descriptor_bytes,
        runtime_manifest,
        instructions_file,
    })
}

#[cfg(unix)]
fn open_relative_file(
    root_fd: &std::os::fd::OwnedFd,
    relative: &Path,
    display_root: &Path,
) -> anyhow::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
    let mut directory = openat(
        root_fd,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        directory = openat(
            &directory,
            component.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
    }
    let name = relative.file_name().context("package file has no name")?;
    let entry_stat = statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(entry_stat.st_mode) == FileType::Symlink {
        anyhow::bail!(
            "skill package cannot contain symlinks: {}",
            display_root.join(relative).display()
        );
    }
    openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .with_context(|| {
        format!(
            "failed to open package file without following symlinks: {}",
            display_root.join(relative).display()
        )
    })
}

fn checked_count(current: u64, maximum: u64, kind: &str) -> anyhow::Result<u64> {
    let total = current
        .checked_add(1)
        .with_context(|| format!("skill package {kind} count overflow"))?;
    if total > maximum {
        anyhow::bail!("skill package {kind} count exceeds {maximum} limit");
    }
    Ok(total)
}

fn checked_bytes(current: u64, bytes: u64, maximum: u64) -> anyhow::Result<u64> {
    let total = current
        .checked_add(bytes)
        .context("skill package byte count overflow")?;
    if total > maximum {
        anyhow::bail!("skill package exceeds {maximum} byte limit");
    }
    Ok(total)
}

#[cfg(not(unix))]
fn snapshot_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<SecurePackageSnapshot> {
    snapshot_fallback(root, limits)
}

#[cfg(not(unix))]
fn hash_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<String> {
    Ok(scan_fallback(root, limits)?.content_hash)
}

#[cfg(not(unix))]
fn tree_direct(root: &Path, limits: PackageLimits) -> anyhow::Result<SecureTreeSnapshot> {
    scan_fallback(root, limits)
}

#[cfg(not(unix))]
fn snapshot_beneath(
    trusted_root: &Path,
    relative: &Path,
    limits: PackageLimits,
) -> anyhow::Result<SecurePackageSnapshot> {
    let root = trusted_root.join(relative);
    let canonical_store = std::fs::canonicalize(trusted_root)?;
    let canonical_root = std::fs::canonicalize(&root)?;
    if !canonical_root.starts_with(&canonical_store) {
        anyhow::bail!(
            "managed package path escapes trusted store root: {}",
            root.display()
        );
    }
    snapshot_fallback(&canonical_root, limits)
}

#[cfg(not(unix))]
fn snapshot_fallback(root: &Path, limits: PackageLimits) -> anyhow::Result<SecurePackageSnapshot> {
    let scanned = scan_fallback(root, limits)?;
    let descriptor = SkillPackageDescriptor::load_from_file_bytes(
        root,
        scanned.descriptor_bytes.clone(),
        scanned.runtime_manifest.clone(),
        scanned.instructions_file.clone(),
    )?;
    Ok(SecurePackageSnapshot {
        descriptor,
        content_hash: scanned.content_hash,
        runtime_manifest: scanned.runtime_manifest,
        instructions_file: scanned.instructions_file,
    })
}

#[cfg(not(unix))]
fn scan_fallback(root: &Path, limits: PackageLimits) -> anyhow::Result<SecureTreeSnapshot> {
    use std::fs::File;
    use std::io::Read;

    struct FallbackFile {
        path: PathBuf,
        relative: PathBuf,
        canonical: Vec<u8>,
        expected_bytes: u64,
    }

    let canonical_root = std::fs::canonicalize(root)?;
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        anyhow::bail!(
            "skill package root must be a real directory: {}",
            root.display()
        );
    }
    let mut stack = vec![PathBuf::new()];
    let mut files = Vec::new();
    let mut portable_paths = BTreeMap::new();
    let mut entries = 0_u64;
    let mut file_count = 0_u64;
    let mut directories = 0_u64;
    let mut package_bytes = 0_u64;
    while let Some(relative_directory) = stack.pop() {
        for entry in std::fs::read_dir(root.join(&relative_directory))? {
            let relative = relative_directory.join(entry?.file_name());
            let canonical = canonical_relative_path(&relative)?;
            if u64::try_from(canonical.len())? > limits.max_relative_path_bytes {
                anyhow::bail!(
                    "skill package relative path exceeds {} byte limit",
                    limits.max_relative_path_bytes
                );
            }
            if u64::try_from(relative.components().count())? > limits.max_depth {
                anyhow::bail!(
                    "skill package path depth exceeds {} component limit",
                    limits.max_depth
                );
            }
            entries = checked_count(entries, limits.max_entries, "entry")?;
            let collision = portable_collision_key(&relative)?;
            register_portable_path(&mut portable_paths, &relative, &collision)?;
            let path = root.join(&relative);
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("skill package cannot contain symlinks: {}", path.display());
            }
            if metadata.is_dir() {
                directories = checked_count(directories, limits.max_directories, "directory")?;
                stack.push(relative);
                continue;
            }
            if !metadata.is_file() {
                anyhow::bail!(
                    "skill package cannot contain special files: {}",
                    path.display()
                );
            }
            if metadata.len() > limits.max_file_bytes {
                anyhow::bail!(
                    "skill package file exceeds {} byte limit: {}",
                    limits.max_file_bytes,
                    path.display()
                );
            }
            package_bytes = checked_bytes(package_bytes, metadata.len(), limits.max_package_bytes)?;
            file_count = checked_count(file_count, limits.max_files, "file")?;
            files.push(FallbackFile {
                path,
                relative,
                canonical,
                expected_bytes: metadata.len(),
            });
        }
    }
    files.sort_by(|left, right| left.canonical.cmp(&right.canonical));
    let mut hasher = Sha256::new();
    hasher.update(TREE_HASH_DOMAIN);
    hasher.update(TREE_HASH_VERSION.to_be_bytes());
    let mut descriptor_bytes = None;
    let mut runtime_manifest = None;
    let mut instructions_file = None;
    for entry in files {
        let mut file = File::open(&entry.path)?;
        let opened_metadata = file.metadata()?;
        let opened_path = std::fs::canonicalize(&entry.path)?;
        if !opened_metadata.is_file() || !opened_path.starts_with(&canonical_root) {
            anyhow::bail!(
                "opened package file escapes package root: {}",
                entry.path.display()
            );
        }
        hasher.update([TREE_HASH_FILE_ENTRY]);
        hasher.update(u64::try_from(entry.canonical.len())?.to_be_bytes());
        hasher.update(&entry.canonical);
        hasher.update(entry.expected_bytes.to_be_bytes());
        let capture = matches!(
            entry.canonical.as_slice(),
            b"general-agent.json" | b"skill.json" | b"SKILL.md"
        );
        let mut captured = capture.then(Vec::new);
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        let mut bytes_read = 0_u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes_read = bytes_read
                .checked_add(u64::try_from(count)?)
                .context("package file length overflowed while hashing")?;
            hasher.update(&buffer[..count]);
            if let Some(bytes) = &mut captured {
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
        if bytes_read != entry.expected_bytes {
            anyhow::bail!(
                "package file changed while hashing: {}",
                entry.path.display()
            );
        }
        match entry.canonical.as_slice() {
            b"general-agent.json" => descriptor_bytes = captured,
            b"skill.json" => runtime_manifest = captured,
            b"SKILL.md" => instructions_file = captured,
            _ => {}
        }
    }
    Ok(SecureTreeSnapshot {
        content_hash: hex::encode(hasher.finalize()),
        descriptor_bytes,
        runtime_manifest,
        instructions_file,
    })
}
