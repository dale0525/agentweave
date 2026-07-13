use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const MARKER_NAME: &str = ".general-agent-initialization.json";
const MARKER_LIMIT: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptPathKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PersistentIdentity {
    kind: AttemptPathKind,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    file_index: u64,
}

impl PersistentIdentity {
    fn from_file(file: &File, kind: AttemptPathKind) -> anyhow::Result<Self> {
        let metadata = file.metadata()?;
        validate_metadata(&metadata, kind)?;
        validate_link_count(&metadata, kind)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                kind,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            Ok(Self {
                kind,
                volume: metadata
                    .volume_serial_number()
                    .context("tenant path has no volume identity")?,
                file_index: metadata
                    .file_index()
                    .context("tenant path has no file identity")?,
            })
        }
        #[cfg(all(not(unix), not(windows)))]
        anyhow::bail!("tenant attempt identities are unsupported on this platform")
    }

    fn from_path(path: &Path, kind: AttemptPathKind) -> anyhow::Result<Self> {
        let file = open_nofollow(path, kind, false)?;
        Self::from_file(&file, kind)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OwnedResource {
    relative_path: String,
    identity: PersistentIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AttemptRecord {
    version: u8,
    token: String,
    root_owned: bool,
    root_identity: PersistentIdentity,
    resources: Vec<OwnedResource>,
}

pub(crate) struct TenantAttemptJournal {
    root: PathBuf,
    marker: PathBuf,
    record: AttemptRecord,
    root_descriptor: File,
    descriptors: HashMap<String, File>,
}

impl TenantAttemptJournal {
    pub(crate) async fn begin(root: PathBuf, root_created: bool) -> anyhow::Result<Self> {
        let carried_root_ownership = recover_stale_attempt(&root).await?;
        let root_descriptor = open_nofollow(&root, AttemptPathKind::Directory, false)?;
        let record = AttemptRecord {
            version: 1,
            token: uuid::Uuid::new_v4().to_string(),
            root_owned: root_created || carried_root_ownership,
            root_identity: PersistentIdentity::from_file(
                &root_descriptor,
                AttemptPathKind::Directory,
            )?,
            resources: Vec::new(),
        };
        let marker = root.join(MARKER_NAME);
        write_record(&marker, &record).await?;
        Ok(Self {
            root,
            marker,
            record,
            root_descriptor,
            descriptors: HashMap::new(),
        })
    }

    pub(crate) async fn create_owned_file(&mut self, path: &Path) -> anyhow::Result<bool> {
        if path.parent() != Some(self.root.as_path()) {
            anyhow::bail!("attempt-owned database must be a direct tenant child");
        }
        let descriptor = match create_file_nofollow(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        self.record_descriptor(path, AttemptPathKind::File, descriptor)
            .await?;
        Ok(true)
    }

    pub(crate) async fn claim_existing(
        &mut self,
        path: &Path,
        kind: AttemptPathKind,
    ) -> anyhow::Result<()> {
        let relative = relative_path(&self.root, path)?;
        if self
            .record
            .resources
            .iter()
            .any(|resource| resource.relative_path == relative)
        {
            return Ok(());
        }
        let descriptor = match open_nofollow(path, kind, false) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        self.record_descriptor(path, kind, descriptor).await
    }

    async fn record_descriptor(
        &mut self,
        path: &Path,
        kind: AttemptPathKind,
        descriptor: File,
    ) -> anyhow::Result<()> {
        let relative = relative_path(&self.root, path)?;
        let identity = PersistentIdentity::from_file(&descriptor, kind)?;
        self.record.resources.push(OwnedResource {
            relative_path: relative.clone(),
            identity,
        });
        if let Err(error) = write_record(&self.marker, &self.record).await {
            self.record.resources.pop();
            return Err(error);
        }
        self.descriptors.insert(relative, descriptor);
        Ok(())
    }

    pub(crate) async fn commit(&self) -> anyhow::Result<()> {
        validate_marker_token(&self.marker, &self.record.token).await?;
        quarantine_marker(&self.marker, &self.record.token).await
    }

    pub(crate) async fn cleanup(&self) {
        if validate_marker_token(&self.marker, &self.record.token)
            .await
            .is_err()
        {
            tracing::warn!("tenant initialization cleanup skipped after marker mismatch");
            return;
        }
        cleanup_resources(self).await;
        if self.record.root_owned && root_contains_only_marker(&self.root, &self.marker) {
            if quarantine_owned_root(self).await.is_ok() {
                return;
            }
            tracing::warn!("failed to quarantine attempt-owned tenant root");
        }
        if let Err(error) = quarantine_marker(&self.marker, &self.record.token).await {
            tracing::warn!(?error, "failed to clean tenant initialization marker");
        }
    }

    #[cfg(test)]
    pub(crate) async fn replace_marker_token_for_test(&self, token: &str) -> anyhow::Result<()> {
        let mut record = self.record.clone();
        record.token = token.to_string();
        write_record(&self.marker, &record).await
    }
}

async fn recover_stale_attempt(root: &Path) -> anyhow::Result<bool> {
    let marker = root.join(MARKER_NAME);
    let Some(record) = read_record_if_present(&marker).await? else {
        return Ok(false);
    };
    let root_descriptor = open_nofollow(root, AttemptPathKind::Directory, false)?;
    let journal = TenantAttemptJournal {
        root: root.to_path_buf(),
        marker: marker.clone(),
        record,
        root_descriptor,
        descriptors: HashMap::new(),
    };
    validate_marker_token(&marker, &journal.record.token).await?;
    cleanup_resources(&journal).await;
    let carry = journal.record.root_owned
        && identity_matches_file(&journal.root_descriptor, &journal.record.root_identity)
        && root_contains_only_marker(root, &marker);
    quarantine_marker(&marker, &journal.record.token).await?;
    Ok(carry)
}

async fn cleanup_resources(journal: &TenantAttemptJournal) {
    let mut resources = journal.record.resources.iter().collect::<Vec<_>>();
    resources
        .sort_by_key(|resource| std::cmp::Reverse(resource.relative_path.matches('/').count()));
    for resource in resources {
        let path = journal.root.join(&resource.relative_path);
        let descriptor = journal.descriptors.get(&resource.relative_path);
        if let Err(error) =
            quarantine_owned_path(&path, &resource.identity, descriptor, &journal.record.token)
                .await
        {
            tracing::warn!(?error, "failed to quarantine attempt-owned tenant path");
        }
    }
}

async fn quarantine_owned_path(
    path: &Path,
    identity: &PersistentIdentity,
    descriptor: Option<&File>,
    token: &str,
) -> anyhow::Result<()> {
    let quarantine = quarantine_path(path, token)?;
    if !path.exists() {
        return remove_existing_quarantine(&quarantine, identity, descriptor).await;
    }
    if PersistentIdentity::from_path(path, identity.kind)? != *identity
        || descriptor.is_some_and(|file| !identity_matches_file(file, identity))
    {
        return Ok(());
    }
    anyhow::ensure!(!quarantine.exists(), "tenant quarantine name is occupied");
    tokio::fs::rename(path, &quarantine).await?;
    if PersistentIdentity::from_path(&quarantine, identity.kind)? != *identity
        || descriptor.is_some_and(|file| !identity_matches_file(file, identity))
    {
        restore_quarantine(&quarantine, path).await;
        anyhow::bail!("tenant path identity changed during quarantine");
    }
    remove_path(&quarantine, identity.kind).await
}

async fn remove_existing_quarantine(
    quarantine: &Path,
    identity: &PersistentIdentity,
    descriptor: Option<&File>,
) -> anyhow::Result<()> {
    if !quarantine.exists() {
        return Ok(());
    }
    if PersistentIdentity::from_path(quarantine, identity.kind)? == *identity
        && descriptor.is_none_or(|file| identity_matches_file(file, identity))
    {
        remove_path(quarantine, identity.kind).await?;
    }
    Ok(())
}

async fn quarantine_owned_root(journal: &TenantAttemptJournal) -> anyhow::Result<()> {
    anyhow::ensure!(
        identity_matches_file(&journal.root_descriptor, &journal.record.root_identity),
        "tenant root descriptor identity changed"
    );
    anyhow::ensure!(
        PersistentIdentity::from_path(&journal.root, AttemptPathKind::Directory)?
            == journal.record.root_identity,
        "tenant root path identity changed"
    );
    let quarantine = quarantine_path(&journal.root, &journal.record.token)?;
    anyhow::ensure!(
        !quarantine.exists(),
        "tenant root quarantine name is occupied"
    );
    tokio::fs::rename(&journal.root, &quarantine).await?;
    let quarantined_marker = quarantine.join(MARKER_NAME);
    if PersistentIdentity::from_path(&quarantine, AttemptPathKind::Directory)?
        != journal.record.root_identity
        || validate_marker_token(&quarantined_marker, &journal.record.token)
            .await
            .is_err()
    {
        restore_quarantine(&quarantine, &journal.root).await;
        anyhow::bail!("tenant root ownership changed during quarantine");
    }
    quarantine_marker(&quarantined_marker, &journal.record.token).await?;
    tokio::fs::remove_dir(&quarantine).await?;
    Ok(())
}

async fn quarantine_marker(marker: &Path, token: &str) -> anyhow::Result<()> {
    validate_marker_token(marker, token).await?;
    let quarantine = marker.with_file_name(format!("{MARKER_NAME}.{token}.quarantine"));
    anyhow::ensure!(
        !quarantine.exists(),
        "tenant marker quarantine name is occupied"
    );
    tokio::fs::rename(marker, &quarantine).await?;
    if validate_marker_token(&quarantine, token).await.is_err() {
        restore_quarantine(&quarantine, marker).await;
        anyhow::bail!("tenant initialization marker changed during quarantine");
    }
    tokio::fs::remove_file(quarantine).await?;
    Ok(())
}

async fn validate_marker_token(marker: &Path, token: &str) -> anyhow::Result<()> {
    let record = read_record(marker).await?;
    anyhow::ensure!(record.token == token, "tenant initialization token changed");
    Ok(())
}

async fn read_record_if_present(path: &Path) -> anyhow::Result<Option<AttemptRecord>> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => read_record(path).await.map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn read_record(path: &Path) -> anyhow::Result<AttemptRecord> {
    let file = open_nofollow(path, AttemptPathKind::File, false)?;
    anyhow::ensure!(
        file.metadata()?.len() <= MARKER_LIMIT,
        "tenant marker is too large"
    );
    let bytes = tokio::fs::read(path).await?;
    anyhow::ensure!(
        bytes.len() as u64 <= MARKER_LIMIT,
        "tenant marker is too large"
    );
    let record: AttemptRecord = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(record.version == 1, "unsupported tenant marker version");
    uuid::Uuid::parse_str(&record.token).context("invalid tenant marker token")?;
    Ok(record)
}

async fn write_record(path: &Path, record: &AttemptRecord) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(record)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MARKER_LIMIT,
        "tenant marker is too large"
    );
    let temporary = path.with_file_name(format!(".{MARKER_NAME}.{}.tmp", uuid::Uuid::new_v4()));
    let mut file = create_file_nofollow(&temporary)?;
    use std::io::Write;
    file.write_all(&bytes)?;
    file.sync_all()?;
    tokio::fs::rename(&temporary, path).await?;
    Ok(())
}

fn open_nofollow(path: &Path, kind: AttemptPathKind, write: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    set_nofollow(&mut options);
    let file = options.open(path)?;
    validate_metadata(&file.metadata()?, kind)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(file)
}

fn create_file_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    set_nofollow(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn set_nofollow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
}

#[cfg(windows)]
fn set_nofollow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(all(not(unix), not(windows)))]
fn set_nofollow(_options: &mut OpenOptions) {}

fn validate_metadata(metadata: &std::fs::Metadata, kind: AttemptPathKind) -> anyhow::Result<()> {
    let valid = !metadata.file_type().is_symlink()
        && match kind {
            AttemptPathKind::File => metadata.is_file(),
            AttemptPathKind::Directory => metadata.is_dir(),
        };
    anyhow::ensure!(valid, "tenant attempt path has an invalid type");
    Ok(())
}

fn validate_link_count(metadata: &std::fs::Metadata, kind: AttemptPathKind) -> anyhow::Result<()> {
    if kind != AttemptPathKind::File {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "tenant attempt file must have one link"
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            metadata.number_of_links() == 1,
            "tenant attempt file must have one link"
        );
    }
    Ok(())
}

fn identity_matches_file(file: &File, expected: &PersistentIdentity) -> bool {
    PersistentIdentity::from_file(file, expected.kind).is_ok_and(|identity| identity == *expected)
}

fn relative_path(root: &Path, path: &Path) -> anyhow::Result<String> {
    let relative = path
        .strip_prefix(root)
        .context("attempt-owned path escaped tenant root")?;
    anyhow::ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_))),
        "attempt-owned path is not canonical"
    );
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn quarantine_path(path: &Path, token: &str) -> anyhow::Result<PathBuf> {
    let name = path
        .file_name()
        .context("tenant attempt path has no file name")?
        .to_string_lossy();
    Ok(path.with_file_name(format!(".{name}.ga-init-{token}.quarantine")))
}

fn root_contains_only_marker(root: &Path, marker: &Path) -> bool {
    std::fs::read_dir(root).is_ok_and(|entries| {
        let paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        paths.len() == 1 && paths[0] == marker
    })
}

async fn remove_path(path: &Path, kind: AttemptPathKind) -> anyhow::Result<()> {
    match kind {
        AttemptPathKind::File => tokio::fs::remove_file(path).await?,
        AttemptPathKind::Directory => tokio::fs::remove_dir(path).await?,
    }
    Ok(())
}

async fn restore_quarantine(quarantine: &Path, original: &Path) {
    if !original.exists()
        && let Err(error) = tokio::fs::rename(quarantine, original).await
    {
        tracing::warn!(
            ?error,
            "failed to restore foreign tenant path from quarantine"
        );
    }
}
