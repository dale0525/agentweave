use crate::tenant_attempt::TenantAttemptJournal;
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
}

pub(crate) struct TenantInitializationPaths {
    pub(crate) data: PreparedTenantDirectory,
    pub(crate) cache: PreparedTenantDirectory,
    attempt: TenantAttemptJournal,
}

impl TenantInitializationPaths {
    pub(crate) async fn capture(
        control_root: PathBuf,
        quarantine_root: PathBuf,
        tenant_id: &str,
        data_parent: &Path,
        cache_parent: &Path,
    ) -> anyhow::Result<Self> {
        let data_path = data_parent.join(tenant_id);
        let cache_path = cache_parent.join(tenant_id);
        let mut attempt = TenantAttemptJournal::begin(
            control_root,
            quarantine_root,
            tenant_id,
            vec![data_parent.to_path_buf(), cache_parent.to_path_buf()],
        )
        .await?;
        let prepared = async {
            attempt.ensure_directory(&data_path).await?;
            attempt.ensure_directory(&cache_path).await?;
            verify_tenant_root(data_parent, &data_path).await?;
            verify_tenant_root(cache_parent, &cache_path).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = prepared {
            if let Err(cleanup_error) = attempt.cleanup().await {
                return Err(error).context(format!(
                    "tenant root preparation failed and owned cleanup was retained: {cleanup_error}"
                ));
            }
            return Err(error).context("tenant root preparation failed");
        }
        Ok(Self {
            data: PreparedTenantDirectory { path: data_path },
            cache: PreparedTenantDirectory { path: cache_path },
            attempt,
        })
    }

    pub(crate) async fn prepare_database(&mut self) -> anyhow::Result<()> {
        self.attempt
            .create_owned_file(&self.data.path.join("state.db"))
            .await?;
        Ok(())
    }

    pub(crate) async fn prepare_store_paths(&mut self) -> anyhow::Result<()> {
        for path in [
            self.data.path.join("app"),
            self.data.path.join("app/managed-skills"),
            self.data.path.join("app/managed-skills/.locks"),
            self.data.path.join("app/skill-quarantine"),
            self.cache.path.join("cache"),
            self.cache.path.join("cache/skill-staging"),
        ] {
            self.attempt.ensure_directory(&path).await?;
        }
        Ok(())
    }

    pub(crate) async fn cleanup(&mut self) -> anyhow::Result<()> {
        self.attempt.cleanup().await
    }

    pub(crate) async fn commit(&mut self) -> anyhow::Result<()> {
        self.attempt.commit().await
    }
}

async fn verify_tenant_root(parent: &Path, path: &Path) -> anyhow::Result<()> {
    ensure_real_directory(path, &tokio::fs::symlink_metadata(path).await?)?;
    anyhow::ensure!(
        tokio::fs::canonicalize(path).await?
            == parent.join(path.file_name().context("tenant root has no name")?),
        "tenant root has a canonical alias instead of the requested tenant id"
    );
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
