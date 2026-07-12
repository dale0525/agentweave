use crate::skill_store_locks::StoreRootIdentity;
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) struct BundlePublisherLock {
    parent: StoreRootIdentity,
    output_relative: PathBuf,
    _descriptor: File,
}

impl BundlePublisherLock {
    pub(crate) fn parent(&self) -> &StoreRootIdentity {
        &self.parent
    }

    pub(crate) fn output_relative(&self) -> &Path {
        &self.output_relative
    }
}

pub(crate) async fn acquire_bundle_publisher_lock(
    output_root: &Path,
) -> anyhow::Result<BundlePublisherLock> {
    let parent = output_root.parent().context("output root has no parent")?;
    tokio::fs::create_dir_all(parent).await?;
    let parent_path = tokio::fs::canonicalize(parent).await?;
    let parent = StoreRootIdentity::capture(parent_path)?;
    let output_name = output_root
        .file_name()
        .context("output root has no file name")?;
    let output_relative = PathBuf::from(output_name);
    let lock_name = publisher_lock_name(output_name);
    let lock_path = parent.path().join(&lock_name);
    let parent_for_worker = parent.clone();
    let output_root = output_root.to_path_buf();
    let descriptor = tokio::task::spawn_blocking(move || {
        let mut descriptor = open_publisher_lock(&parent_for_worker, &lock_name)?;
        wait_for_publisher_lock(&mut descriptor, &lock_path)?;
        validate_publisher_lock(&parent_for_worker, &lock_name, &descriptor)?;
        write_lock_diagnostic(&mut descriptor, &output_root)?;
        parent_for_worker.verify("bundle publisher lock parent")?;
        #[cfg(test)]
        subprocess_after_lock_checkpoint()?;
        Ok::<_, anyhow::Error>(descriptor)
    })
    .await
    .context("bundle publisher lock worker failed")??;
    Ok(BundlePublisherLock {
        parent,
        output_relative,
        _descriptor: descriptor,
    })
}

#[cfg(test)]
fn subprocess_after_lock_checkpoint() -> anyhow::Result<()> {
    let Some(marker) = std::env::var_os("GENERAL_AGENT_TEST_BUNDLE_LOCK_MARKER") else {
        return Ok(());
    };
    let release = std::env::var_os("GENERAL_AGENT_TEST_BUNDLE_LOCK_RELEASE")
        .context("missing subprocess bundle lock release path")?;
    std::fs::write(marker, b"locked")?;
    let started = Instant::now();
    while !Path::new(&release).exists() {
        anyhow::ensure!(
            started.elapsed() < LOCK_WAIT_TIMEOUT,
            "timed out waiting for subprocess bundle lock release"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn wait_for_publisher_lock(descriptor: &mut File, lock_path: &Path) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(descriptor) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() >= LOCK_WAIT_TIMEOUT {
                    let holder = read_lock_diagnostic(descriptor)
                        .unwrap_or_else(|error| format!("unavailable ({error})"));
                    anyhow::bail!(
                        "timed out after {}s waiting for bundle publisher lock {} (holder: {})",
                        LOCK_WAIT_TIMEOUT.as_secs(),
                        lock_path.display(),
                        holder.trim()
                    );
                }
                std::thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to acquire bundle publisher lock {}",
                        lock_path.display()
                    )
                });
            }
        }
    }
}

fn write_lock_diagnostic(descriptor: &mut File, output_root: &Path) -> anyhow::Result<()> {
    descriptor.set_len(0)?;
    descriptor.rewind()?;
    writeln!(
        descriptor,
        "pid={} output={}",
        std::process::id(),
        output_root.display()
    )?;
    descriptor.sync_data()?;
    Ok(())
}

fn read_lock_diagnostic(descriptor: &File) -> anyhow::Result<String> {
    let mut copy = descriptor.try_clone()?;
    copy.rewind()?;
    let mut diagnostic = String::new();
    copy.take(4096).read_to_string(&mut diagnostic)?;
    Ok(diagnostic)
}

fn publisher_lock_name(output_name: &OsStr) -> OsString {
    let mut digest = Sha256::new();
    update_path_digest(&mut digest, output_name);
    OsString::from(format!(
        ".skill-bundle-publisher-{}.lock",
        hex::encode(digest.finalize())
    ))
}

#[cfg(unix)]
fn update_path_digest(digest: &mut Sha256, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;
    digest.update(value.as_bytes());
}

#[cfg(windows)]
fn update_path_digest(digest: &mut Sha256, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt;
    for unit in value.encode_wide() {
        digest.update(unit.to_le_bytes());
    }
}

#[cfg(all(not(unix), not(windows)))]
fn update_path_digest(digest: &mut Sha256, value: &OsStr) {
    digest.update(value.to_string_lossy().as_bytes());
}

#[cfg(unix)]
fn open_publisher_lock(parent: &StoreRootIdentity, name: &OsStr) -> anyhow::Result<File> {
    use rustix::fs::{FileType, Mode, OFlags, RawMode, fstat, openat};
    let descriptor = openat(
        parent.descriptor(),
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(RawMode::try_from(0o600_u32)?),
    )?;
    let stat = fstat(&descriptor)?;
    anyhow::ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile,
        "bundle publisher lock is not a regular file"
    );
    anyhow::ensure!(
        stat.st_nlink == 1,
        "bundle publisher lock must have exactly one link"
    );
    Ok(descriptor.into())
}

#[cfg(unix)]
fn validate_publisher_lock(
    parent: &StoreRootIdentity,
    name: &OsStr,
    descriptor: &File,
) -> anyhow::Result<()> {
    use rustix::fs::{AtFlags, FileType, fstat, statat};
    let locked = fstat(descriptor)?;
    let entry = statat(parent.descriptor(), name, AtFlags::SYMLINK_NOFOLLOW)?;
    anyhow::ensure!(
        FileType::from_raw_mode(entry.st_mode) == FileType::RegularFile
            && entry.st_nlink == 1
            && entry.st_dev == locked.st_dev
            && entry.st_ino == locked.st_ino,
        "bundle publisher lock directory entry changed after open"
    );
    Ok(())
}

#[cfg(windows)]
fn open_publisher_lock(parent: &StoreRootIdentity, name: &OsStr) -> anyhow::Result<File> {
    crate::skill_store_windows::open_lock_file_beneath(
        parent.windows_descriptor(),
        parent.windows_identity(),
        name,
    )
}

#[cfg(windows)]
fn validate_publisher_lock(
    parent: &StoreRootIdentity,
    name: &OsStr,
    descriptor: &File,
) -> anyhow::Result<()> {
    crate::skill_store_windows::validate_lock_file_beneath(
        parent.windows_descriptor(),
        parent.windows_identity(),
        name,
        descriptor,
    )
}

#[cfg(all(not(unix), not(windows)))]
fn open_publisher_lock(parent: &StoreRootIdentity, name: &OsStr) -> anyhow::Result<File> {
    parent.verify("bundle publisher lock parent")?;
    let path = parent.path().join(name);
    let descriptor = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)?;
    anyhow::ensure!(
        descriptor.metadata()?.is_file(),
        "bundle publisher lock is not a file"
    );
    Ok(descriptor)
}

#[cfg(all(not(unix), not(windows)))]
fn validate_publisher_lock(
    parent: &StoreRootIdentity,
    name: &OsStr,
    descriptor: &File,
) -> anyhow::Result<()> {
    parent.verify("bundle publisher lock parent")?;
    let expected = same_file::Handle::from_file(descriptor.try_clone()?)?;
    let current = same_file::Handle::from_path(parent.path().join(name))?;
    anyhow::ensure!(
        expected == current,
        "bundle publisher lock entry changed after open"
    );
    Ok(())
}
