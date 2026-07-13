use crate::tenant_attempt::{AttemptPathKind, TenantAttemptJournal};
use anyhow::Context;
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) struct TenantInitializationLock {
    _descriptor: File,
}

pub(crate) async fn acquire_tenant_initialization_lock(
    lock_root: &Path,
    tenant_id: &str,
) -> anyhow::Result<TenantInitializationLock> {
    let root = DirectoryIdentity::capture(lock_root.to_path_buf())
        .context("tenant lock root identity capture failed")?;
    let lock_name = format!("{tenant_id}.lock");
    let lock_path = lock_root.join(&lock_name);
    let descriptor = tokio::task::spawn_blocking(move || {
        let descriptor =
            open_lock_file(&root, OsStr::new(&lock_name)).context("tenant lock open failed")?;
        wait_for_lock(&descriptor, &lock_path).context("tenant lock wait failed")?;
        validate_lock_file(&root, OsStr::new(&lock_name), &descriptor)
            .context("tenant lock entry validation failed")?;
        root.verify("tenant initialization lock root")
            .context("tenant lock root revalidation failed")?;
        Ok::<_, anyhow::Error>(descriptor)
    })
    .await
    .context("tenant initialization lock worker failed")??;
    Ok(TenantInitializationLock {
        _descriptor: descriptor,
    })
}

fn wait_for_lock(descriptor: &File, path: &Path) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(descriptor) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                anyhow::ensure!(
                    started.elapsed() < LOCK_WAIT_TIMEOUT,
                    "timed out waiting for tenant initialization lock: {}",
                    path.display()
                );
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to acquire tenant initialization lock: {}",
                        path.display()
                    )
                });
            }
        }
    }
}

#[cfg(unix)]
fn open_lock_file(root: &DirectoryIdentity, name: &OsStr) -> anyhow::Result<File> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawMode, fstat, openat, statat};
    let mut flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    match statat(root.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => anyhow::ensure!(
            FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1,
            "tenant initialization lock must be a one-link regular file"
        ),
        Err(error) if error == rustix::io::Errno::NOENT => flags |= OFlags::CREATE,
        Err(error) => return Err(error.into()),
    }
    let descriptor = match openat(
        root.descriptor(),
        name,
        flags,
        Mode::from_raw_mode(RawMode::try_from(0o600_u32)?),
    ) {
        Ok(descriptor) => descriptor,
        Err(error) if error == rustix::io::Errno::NOENT && flags.contains(OFlags::CREATE) => {
            let entry = statat(root.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW)?;
            anyhow::ensure!(
                FileType::from_raw_mode(entry.st_mode) == FileType::RegularFile
                    && entry.st_nlink == 1,
                "tenant initialization lock must be a one-link regular file"
            );
            openat(
                root.descriptor(),
                name,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )?
        }
        Err(error) => return Err(error.into()),
    };
    let stat = fstat(&descriptor)?;
    anyhow::ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1,
        "tenant initialization lock must be a one-link regular file"
    );
    Ok(descriptor.into())
}

#[cfg(unix)]
fn validate_lock_file(
    root: &DirectoryIdentity,
    name: &OsStr,
    descriptor: &File,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, fstat, statat};
    let opened = fstat(descriptor)?;
    let entry = statat(root.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW)?;
    anyhow::ensure!(
        FileType::from_raw_mode(entry.st_mode) == FileType::RegularFile
            && entry.st_nlink == 1
            && entry.st_dev == opened.st_dev
            && entry.st_ino == opened.st_ino,
        "tenant initialization lock entry changed after open"
    );
    Ok(())
}

#[cfg(windows)]
fn open_lock_file(root: &DirectoryIdentity, name: &OsStr) -> anyhow::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    root.verify("tenant initialization lock root")?;
    let descriptor = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root.path.join(name))?;
    validate_regular_one_link(&descriptor)?;
    Ok(descriptor)
}

#[cfg(windows)]
fn validate_lock_file(
    root: &DirectoryIdentity,
    name: &OsStr,
    descriptor: &File,
) -> anyhow::Result<()> {
    root.verify("tenant initialization lock root")?;
    validate_regular_one_link(descriptor)?;
    let opened = same_file::Handle::from_file(descriptor.try_clone()?)?;
    let entry = same_file::Handle::from_path(root.path.join(name))?;
    anyhow::ensure!(
        opened == entry,
        "tenant initialization lock entry changed after open"
    );
    Ok(())
}

#[cfg(windows)]
fn validate_regular_one_link(descriptor: &File) -> anyhow::Result<()> {
    use std::os::windows::fs::MetadataExt;
    let metadata = descriptor.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.number_of_links() == 1,
        "tenant initialization lock must be a one-link regular file"
    );
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn open_lock_file(_root: &DirectoryIdentity, _name: &OsStr) -> anyhow::Result<File> {
    anyhow::bail!("tenant initialization locks are unsupported on this platform")
}

#[cfg(all(not(unix), not(windows)))]
fn validate_lock_file(
    _root: &DirectoryIdentity,
    _name: &OsStr,
    _descriptor: &File,
) -> anyhow::Result<()> {
    anyhow::bail!("tenant initialization locks are unsupported on this platform")
}

pub(crate) async fn prepare_real_directory(path: &Path) -> anyhow::Result<PathBuf> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => ensure_real_directory(path, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(path).await?;
            ensure_real_directory(path, &tokio::fs::symlink_metadata(path).await?)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(tokio::fs::canonicalize(path).await?)
}

pub(crate) struct PreparedTenantDirectory {
    pub(crate) path: PathBuf,
    created: Option<CreatedPath>,
}

pub(crate) async fn prepare_real_tenant_child(
    parent: &Path,
    tenant_id: &str,
) -> anyhow::Result<PreparedTenantDirectory> {
    let child = parent.join(tenant_id);
    let created = match tokio::fs::symlink_metadata(&child).await {
        Ok(metadata) => {
            ensure_real_directory(&child, &metadata)?;
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::create_dir(&child).await {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    };
    ensure_real_directory(&child, &tokio::fs::symlink_metadata(&child).await?)?;
    let canonical = tokio::fs::canonicalize(&child).await?;
    anyhow::ensure!(
        canonical == parent.join(tenant_id),
        "tenant root has a canonical alias instead of the requested tenant id"
    );
    let created = created
        .then(|| CreatedPath::capture(canonical.clone(), PathKind::Directory))
        .transpose()?;
    Ok(PreparedTenantDirectory {
        path: canonical,
        created,
    })
}

pub(crate) struct TenantInitializationPaths {
    pub(crate) data: PreparedTenantDirectory,
    pub(crate) cache: PreparedTenantDirectory,
    tracked: Vec<TrackedPath>,
    data_attempt: TenantAttemptJournal,
    cache_attempt: TenantAttemptJournal,
}

impl TenantInitializationPaths {
    pub(crate) async fn capture(
        data: PreparedTenantDirectory,
        cache: PreparedTenantDirectory,
    ) -> anyhow::Result<Self> {
        let data_created = data.created.is_some();
        let cache_created = cache.created.is_some();
        let data_attempt = TenantAttemptJournal::begin(data.path.clone(), data_created).await?;
        let cache_attempt =
            match TenantAttemptJournal::begin(cache.path.clone(), cache_created).await {
                Ok(attempt) => attempt,
                Err(error) => {
                    data_attempt.cleanup().await;
                    return Err(error);
                }
            };
        let tracked = [
            (data.path.join("state.db"), PathKind::File),
            (data.path.join("state.db-wal"), PathKind::File),
            (data.path.join("state.db-shm"), PathKind::File),
            (data.path.join("app"), PathKind::Directory),
            (data.path.join("app/managed-skills"), PathKind::Directory),
            (
                data.path.join("app/managed-skills/.locks"),
                PathKind::Directory,
            ),
            (data.path.join("app/skill-quarantine"), PathKind::Directory),
            (cache.path.join("cache"), PathKind::Directory),
            (cache.path.join("cache/skill-staging"), PathKind::Directory),
        ];
        let mut paths = Vec::with_capacity(tracked.len());
        for (path, kind) in tracked {
            paths.push(TrackedPath::capture(path, kind).await?);
        }
        Ok(Self {
            data,
            cache,
            tracked: paths,
            data_attempt,
            cache_attempt,
        })
    }

    pub(crate) async fn prepare_database(&mut self) -> anyhow::Result<()> {
        self.data_attempt
            .create_owned_file(&self.data.path.join("state.db"))
            .await?;
        self.record_created_paths().await
    }

    pub(crate) async fn record_created_paths(&mut self) -> anyhow::Result<()> {
        for tracked in &mut self.tracked {
            let attempt = if tracked.path.starts_with(&self.data.path) {
                &mut self.data_attempt
            } else {
                &mut self.cache_attempt
            };
            tracked.record_created(attempt).await?;
        }
        Ok(())
    }

    pub(crate) async fn cleanup(&self) {
        self.cache_attempt.cleanup().await;
        self.data_attempt.cleanup().await;
    }

    pub(crate) async fn commit(&self) -> anyhow::Result<()> {
        self.data_attempt.commit().await?;
        self.cache_attempt.commit().await?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathKind {
    File,
    Directory,
}

struct TrackedPath {
    path: PathBuf,
    kind: PathKind,
    existed: bool,
    claimed: bool,
}

impl TrackedPath {
    async fn capture(path: PathBuf, kind: PathKind) -> anyhow::Result<Self> {
        let existed = match tokio::fs::symlink_metadata(&path).await {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            kind,
            existed,
            claimed: false,
        })
    }

    async fn record_created(&mut self, attempt: &mut TenantAttemptJournal) -> anyhow::Result<()> {
        if self.existed || self.claimed {
            return Ok(());
        }
        match tokio::fs::symlink_metadata(&self.path).await {
            Ok(metadata) => {
                validate_kind(&self.path, &metadata, self.kind)?;
                attempt.claim_existing(&self.path, self.kind.into()).await?;
                self.claimed = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

impl From<PathKind> for AttemptPathKind {
    fn from(value: PathKind) -> Self {
        match value {
            PathKind::File => Self::File,
            PathKind::Directory => Self::Directory,
        }
    }
}

struct CreatedPath {
    path: PathBuf,
    kind: PathKind,
    identity: same_file::Handle,
    _creation_token: uuid::Uuid,
}

impl CreatedPath {
    fn capture(path: PathBuf, kind: PathKind) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)?;
        validate_kind(&path, &metadata, kind)?;
        let descriptor = File::open(&path)?;
        validate_one_link_file(&descriptor, kind)?;
        Ok(Self {
            path,
            kind,
            identity: same_file::Handle::from_file(descriptor)?,
            _creation_token: uuid::Uuid::new_v4(),
        })
    }

    async fn remove_if_same(&self) {
        let same = tokio::fs::symlink_metadata(&self.path)
            .await
            .ok()
            .filter(|metadata| {
                validate_kind(&self.path, metadata, self.kind).is_ok()
                    && validate_path_link_count(metadata, self.kind).is_ok()
            })
            .and_then(|_| same_file::Handle::from_path(&self.path).ok())
            .is_some_and(|identity| identity == self.identity);
        if !same {
            return;
        }
        let result = match self.kind {
            PathKind::File => tokio::fs::remove_file(&self.path).await,
            PathKind::Directory => tokio::fs::remove_dir(&self.path).await,
        };
        if let Err(error) = result
            && !matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            )
        {
            tracing::warn!("failed to clean tenant initialization path");
        }
    }
}

async fn remove_created_directory(directory: &PreparedTenantDirectory) {
    if let Some(created) = &directory.created {
        created.remove_if_same().await;
    }
}

pub(crate) async fn cleanup_prepared_directory(directory: &PreparedTenantDirectory) {
    remove_created_directory(directory).await;
}

fn validate_kind(path: &Path, metadata: &std::fs::Metadata, kind: PathKind) -> anyhow::Result<()> {
    let valid = !metadata.file_type().is_symlink()
        && match kind {
            PathKind::File => metadata.is_file(),
            PathKind::Directory => metadata.is_dir(),
        };
    anyhow::ensure!(
        valid,
        "tenant initialization path has an invalid type: {}",
        path.display()
    );
    Ok(())
}

fn validate_one_link_file(descriptor: &File, kind: PathKind) -> anyhow::Result<()> {
    if kind != PathKind::File {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            descriptor.metadata()?.nlink() == 1,
            "tenant file must have one link"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            descriptor.metadata()?.number_of_links() == 1,
            "tenant file must have one link"
        );
    }
    Ok(())
}

fn validate_path_link_count(metadata: &std::fs::Metadata, kind: PathKind) -> anyhow::Result<()> {
    if kind != PathKind::File {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(metadata.nlink() == 1, "tenant file must have one link");
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            metadata.number_of_links() == 1,
            "tenant file must have one link"
        );
    }
    Ok(())
}

fn ensure_real_directory(path: &Path, metadata: &std::fs::Metadata) -> anyhow::Result<()> {
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "tenant storage path must be a real directory: {}",
        path.display()
    );
    Ok(())
}

#[derive(Clone)]
struct DirectoryIdentity {
    path: PathBuf,
    identity: std::sync::Arc<same_file::Handle>,
    #[cfg(unix)]
    descriptor: std::sync::Arc<File>,
}

impl DirectoryIdentity {
    fn capture(path: PathBuf) -> anyhow::Result<Self> {
        let metadata = std::fs::symlink_metadata(&path)?;
        ensure_real_directory(&path, &metadata)?;
        let descriptor = File::open(&path)?;
        let identity = same_file::Handle::from_file(descriptor.try_clone()?)?;
        Ok(Self {
            path,
            identity: std::sync::Arc::new(identity),
            #[cfg(unix)]
            descriptor: std::sync::Arc::new(descriptor),
        })
    }

    fn verify(&self, label: &str) -> anyhow::Result<()> {
        let current = same_file::Handle::from_path(&self.path)
            .with_context(|| format!("failed to verify {label}"))?;
        anyhow::ensure!(current == *self.identity, "{label} identity changed");
        Ok(())
    }

    #[cfg(unix)]
    fn descriptor(&self) -> &File {
        &self.descriptor
    }
}
