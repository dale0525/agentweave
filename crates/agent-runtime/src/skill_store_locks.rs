use anyhow::Context;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::skill_store_secure_fs::ensure_store_directory;

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

    fn managed_path(&self) -> &Path {
        &self.managed.path
    }

    #[cfg(not(unix))]
    fn locks_path(&self) -> &Path {
        &self.locks.path
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct StoreRootIdentity {
    path: PathBuf,
    handle: Arc<same_file::Handle>,
}

impl StoreRootIdentity {
    fn capture(path: PathBuf) -> anyhow::Result<Self> {
        let handle = same_file::Handle::from_path(&path)?;
        Ok(Self {
            path,
            handle: Arc::new(handle),
        })
    }

    fn verify(&self, label: &str) -> anyhow::Result<()> {
        let current = same_file::Handle::from_path(&self.path)
            .with_context(|| format!("failed to verify {label} store root identity"))?;
        if current != *self.handle {
            anyhow::bail!(
                "{label} store root identity changed: {}",
                self.path.display()
            );
        }
        Ok(())
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
    store.verify()?;
    ensure_store_directory(store.managed_path(), Path::new(".locks")).await?;
    let store = store.clone();
    let revision_id = revision_id.to_string();
    tokio::task::spawn_blocking(move || {
        use fs2::FileExt;
        store.verify()?;
        let descriptor = open_revision_lock_file(&store, &revision_id)?;
        descriptor
            .lock_exclusive()
            .with_context(|| format!("failed to lock revision operation: {revision_id}"))?;
        store.verify()?;
        Ok(OsRevisionLock {
            _descriptor: descriptor,
        })
    })
    .await
    .context("revision lock worker failed")?
}

#[cfg(unix)]
fn open_revision_lock_file(store: &SkillStoreIdentity, revision_id: &str) -> anyhow::Result<File> {
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, open, openat};
    let root = open(
        store.managed_path(),
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
    Ok(descriptor.into())
}

#[cfg(not(unix))]
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
