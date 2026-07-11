use anyhow::Context;
use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SkillStoreIdentity {
    managed: StoreRootIdentity,
    staging: StoreRootIdentity,
    quarantine: StoreRootIdentity,
    locks: StoreRootIdentity,
}

impl SkillStoreIdentity {
    pub(crate) fn capture(
        managed: PathBuf,
        staging: PathBuf,
        quarantine: PathBuf,
    ) -> anyhow::Result<Self> {
        let locks = managed.join(".locks");
        Ok(Self {
            managed: StoreRootIdentity::capture(managed)?,
            staging: StoreRootIdentity::capture(staging)?,
            quarantine: StoreRootIdentity::capture(quarantine)?,
            locks: StoreRootIdentity::capture(locks)?,
        })
    }

    pub(crate) fn verify(&self) -> anyhow::Result<()> {
        self.managed.verify("managed")?;
        self.staging.verify("staging")?;
        self.quarantine.verify("quarantine")?;
        self.locks.verify("locks")
    }

    fn locks(&self) -> &StoreRootIdentity {
        &self.locks
    }

    pub(crate) fn managed(&self) -> &StoreRootIdentity {
        &self.managed
    }

    pub(crate) fn staging(&self) -> &StoreRootIdentity {
        &self.staging
    }

    pub(crate) fn quarantine(&self) -> &StoreRootIdentity {
        &self.quarantine
    }

    #[cfg(all(not(unix), not(windows)))]
    fn locks_path(&self) -> &Path {
        &self.locks.path
    }
}

#[derive(Clone, Debug)]
pub(crate) struct StoreRootIdentity {
    path: PathBuf,
    handle: Arc<same_file::Handle>,
    #[cfg(unix)]
    descriptor: Arc<File>,
    #[cfg(windows)]
    descriptor: Arc<File>,
    #[cfg(windows)]
    windows_identity: crate::skill_store_windows::WindowsFileIdentity,
}

impl StoreRootIdentity {
    fn capture(path: PathBuf) -> anyhow::Result<Self> {
        #[cfg(unix)]
        let descriptor = File::open(&path)?;
        #[cfg(unix)]
        let handle = same_file::Handle::from_file(descriptor.try_clone()?)?;
        #[cfg(windows)]
        let (descriptor, windows_identity, _) =
            crate::skill_store_windows::open_directory_nofollow(&path)?;
        #[cfg(windows)]
        let handle = same_file::Handle::from_file(descriptor.try_clone()?)?;
        #[cfg(all(not(unix), not(windows)))]
        let handle = same_file::Handle::from_path(&path)?;
        Ok(Self {
            path,
            handle: Arc::new(handle),
            #[cfg(unix)]
            descriptor: Arc::new(descriptor),
            #[cfg(windows)]
            descriptor: Arc::new(descriptor),
            #[cfg(windows)]
            windows_identity,
        })
    }

    pub(crate) fn verify(&self, label: &str) -> anyhow::Result<()> {
        #[cfg(windows)]
        {
            crate::skill_store_windows::verify_directory_path(&self.path, self.windows_identity)
                .with_context(|| format!("failed to verify {label} store root identity"))?;
            return Ok(());
        }
        #[cfg(not(windows))]
        let current = same_file::Handle::from_path(&self.path)
            .with_context(|| format!("failed to verify {label} store root identity"))?;
        #[cfg(not(windows))]
        if current != *self.handle {
            anyhow::bail!(
                "{label} store root identity changed: {}",
                self.path.display()
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn descriptor(&self) -> &File {
        &self.descriptor
    }

    #[cfg(windows)]
    pub(crate) fn windows_descriptor(&self) -> &File {
        &self.descriptor
    }

    #[cfg(windows)]
    pub(crate) fn windows_identity(&self) -> crate::skill_store_windows::WindowsFileIdentity {
        self.windows_identity
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl PartialEq for StoreRootIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.handle == other.handle
    }
}

impl Eq for StoreRootIdentity {}

impl Hash for StoreRootIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.path.hash(state);
        self.handle.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RevisionLockKey {
    store: SkillStoreIdentity,
    revision_id: String,
}

type RevisionLockMap = HashMap<RevisionLockKey, Weak<AsyncMutex<()>>>;

fn revision_locks() -> &'static Mutex<RevisionLockMap> {
    static LOCKS: OnceLock<Mutex<RevisionLockMap>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) async fn acquire_revision_lock(
    store: &SkillStoreIdentity,
    revision_id: &str,
    faults: &StoreFaults,
) -> anyhow::Result<RevisionOperationGuard> {
    let key = RevisionLockKey {
        store: store.clone(),
        revision_id: revision_id.to_string(),
    };
    let lock = {
        let mut locks = revision_locks()
            .lock()
            .expect("revision lock registry poisoned");
        locks.retain(|_, lock| lock.strong_count() > 0);
        match locks.get(&key).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = std::sync::Arc::new(AsyncMutex::new(()));
                locks.insert(key, std::sync::Arc::downgrade(&lock));
                lock
            }
        }
    };
    faults
        .checkpoint(StoreFaultPoint::RevisionLockAttempt)
        .await;
    let process = lock.lock_owned().await;
    let os = acquire_os_revision_lock(store, revision_id).await?;
    #[cfg(test)]
    subprocess_after_lock_checkpoint().await?;
    Ok(RevisionOperationGuard {
        _process: process,
        _os: os,
    })
}

#[cfg(test)]
async fn subprocess_after_lock_checkpoint() -> anyhow::Result<()> {
    let Some(marker) = std::env::var_os("GENERAL_AGENT_TEST_AFTER_LOCK_MARKER") else {
        return Ok(());
    };
    let release = std::env::var_os("GENERAL_AGENT_TEST_AFTER_LOCK_RELEASE")
        .context("missing subprocess lock release path")?;
    tokio::fs::write(marker, b"locked").await?;
    while !Path::new(&release).exists() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Ok(())
}

pub(crate) struct RevisionOperationGuard {
    _process: OwnedMutexGuard<()>,
    _os: OsRevisionLock,
}

pub(crate) struct OsRevisionLock {
    _descriptor: File,
}

pub(crate) async fn acquire_os_revision_lock(
    store: &SkillStoreIdentity,
    revision_id: &str,
) -> anyhow::Result<OsRevisionLock> {
    acquire_os_revision_lock_inner(store, revision_id, None, None).await
}

#[cfg(test)]
pub(crate) async fn acquire_os_revision_lock_with_attempt(
    store: &SkillStoreIdentity,
    revision_id: &str,
    attempt: tokio::sync::oneshot::Sender<()>,
) -> anyhow::Result<OsRevisionLock> {
    acquire_os_revision_lock_inner(store, revision_id, Some(attempt), None).await
}

#[cfg(test)]
pub(crate) async fn acquire_os_revision_lock_with_opened_gate(
    store: &SkillStoreIdentity,
    revision_id: &str,
    opened: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) -> anyhow::Result<OsRevisionLock> {
    acquire_os_revision_lock_inner(store, revision_id, None, Some((opened, release))).await
}

async fn acquire_os_revision_lock_inner(
    store: &SkillStoreIdentity,
    revision_id: &str,
    attempt: Option<tokio::sync::oneshot::Sender<()>>,
    opened_gate: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
) -> anyhow::Result<OsRevisionLock> {
    store.verify()?;
    let store = store.clone();
    let revision_id = revision_id.to_string();
    tokio::task::spawn_blocking(move || {
        use fs2::FileExt;
        store.verify()?;
        let descriptor = open_revision_lock_file(&store, &revision_id)?;
        if let Some((opened, release)) = opened_gate {
            let _ = opened.send(());
            release
                .recv()
                .context("revision lock replacement gate was dropped")?;
        }
        if let Some(attempt) = attempt {
            let _ = attempt.send(());
        }
        descriptor
            .lock_exclusive()
            .with_context(|| format!("failed to lock revision operation: {revision_id}"))?;
        validate_locked_revision_entry(&store, &revision_id, &descriptor)?;
        store.verify()?;
        Ok(OsRevisionLock {
            _descriptor: descriptor,
        })
    })
    .await
    .context("revision lock worker failed")?
}

#[cfg(unix)]
fn validate_locked_revision_entry(
    store: &SkillStoreIdentity,
    revision_id: &str,
    descriptor: &File,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, fstat, statat};
    let locked = fstat(descriptor)?;
    if FileType::from_raw_mode(locked.st_mode) != FileType::RegularFile {
        anyhow::bail!("revision lock path is not a regular file: {revision_id}");
    }
    if locked.st_nlink != 1 {
        anyhow::bail!("revision lock file must have exactly one link: {revision_id}");
    }
    let entry = statat(
        store.locks().descriptor(),
        format!("{revision_id}.lock"),
        AtFlags::SYMLINK_NOFOLLOW,
    )?;
    if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile
        || entry.st_nlink != 1
        || entry.st_dev != locked.st_dev
        || entry.st_ino != locked.st_ino
    {
        anyhow::bail!("revision lock directory entry changed after open: {revision_id}");
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_locked_revision_entry(
    _store: &SkillStoreIdentity,
    _revision_id: &str,
    _descriptor: &File,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn open_revision_lock_file(store: &SkillStoreIdentity, revision_id: &str) -> anyhow::Result<File> {
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, openat};
    let descriptor = openat(
        store.locks().descriptor(),
        format!("{revision_id}.lock"),
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(0o600_u32)?),
    )?;
    let stat = fstat(&descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        anyhow::bail!("revision lock path is not a regular file: {revision_id}");
    }
    Ok(descriptor.into())
}

#[cfg(windows)]
fn open_revision_lock_file(store: &SkillStoreIdentity, revision_id: &str) -> anyhow::Result<File> {
    crate::skill_store_windows::open_lock_file_beneath(
        store.locks().windows_descriptor(),
        store.locks().windows_identity(),
        std::ffi::OsStr::new(&format!("{revision_id}.lock")),
    )
}

#[cfg(all(not(unix), not(windows)))]
fn open_revision_lock_file(store: &SkillStoreIdentity, revision_id: &str) -> anyhow::Result<File> {
    store.verify()?;
    let path = store.locks_path().join(format!("{revision_id}.lock"));
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("revision lock path must not be a symlink: {revision_id}")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let descriptor = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)?;
    if !descriptor.metadata()?.is_file() {
        anyhow::bail!("revision lock path is not a regular file: {revision_id}");
    }
    let canonical = std::fs::canonicalize(&path)?;
    let canonical_locks = std::fs::canonicalize(store.locks_path())?;
    if !canonical.starts_with(canonical_locks) {
        anyhow::bail!("revision lock path escapes locks root: {revision_id}");
    }
    store.verify()?;
    Ok(descriptor)
}
