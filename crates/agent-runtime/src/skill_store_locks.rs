use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::skill_store_secure_fs::ensure_store_directory;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SkillStoreIdentity {
    managed: PathBuf,
    staging: PathBuf,
    quarantine: PathBuf,
}

impl SkillStoreIdentity {
    pub(crate) fn new(managed: PathBuf, staging: PathBuf, quarantine: PathBuf) -> Self {
        Self {
            managed,
            staging,
            quarantine,
        }
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
    let process = lock.lock_owned().await;
    let os = acquire_os_revision_lock(&store.managed, revision_id).await?;
    Ok(RevisionOperationGuard {
        _process: process,
        _os: os,
    })
}

pub(crate) struct RevisionOperationGuard {
    _process: OwnedMutexGuard<()>,
    _os: OsRevisionLock,
}

#[cfg(unix)]
pub(crate) struct OsRevisionLock {
    _descriptor: std::os::fd::OwnedFd,
}

#[cfg(not(unix))]
pub(crate) struct OsRevisionLock;

pub(crate) async fn acquire_os_revision_lock(
    managed_root: &Path,
    revision_id: &str,
) -> anyhow::Result<OsRevisionLock> {
    ensure_store_directory(managed_root, Path::new(".locks")).await?;
    acquire_os_revision_lock_platform(managed_root, revision_id).await
}

#[cfg(unix)]
async fn acquire_os_revision_lock_platform(
    managed_root: &Path,
    revision_id: &str,
) -> anyhow::Result<OsRevisionLock> {
    use anyhow::Context;
    use rustix::fs::{FileType, FlockOperation, Mode, OFlags, RawMode, flock, fstat, open, openat};

    let managed_root = managed_root.to_path_buf();
    let revision_id = revision_id.to_string();
    tokio::task::spawn_blocking(move || {
        let root = open(
            &managed_root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let locks = openat(
            &root,
            ".locks",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        let descriptor = openat(
            &locks,
            format!("{revision_id}.lock"),
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(RawMode::try_from(0o600_u32)?),
        )?;
        let stat = fstat(&descriptor)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            anyhow::bail!("revision lock path is not a regular file: {revision_id}");
        }
        flock(&descriptor, FlockOperation::LockExclusive)
            .with_context(|| format!("failed to lock revision operation: {revision_id}"))?;
        Ok(OsRevisionLock {
            _descriptor: descriptor,
        })
    })
    .await
    .context("revision lock worker failed")?
}

#[cfg(not(unix))]
async fn acquire_os_revision_lock_platform(
    _managed_root: &Path,
    _revision_id: &str,
) -> anyhow::Result<OsRevisionLock> {
    Ok(OsRevisionLock)
}
