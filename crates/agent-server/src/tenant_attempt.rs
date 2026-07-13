use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

#[path = "tenant_attempt_fs.rs"]
mod fs;
#[cfg(test)]
use fs::replace_quarantine_for_test;
use fs::{
    canonical_real_directory, create_private_object, open_nofollow, read_object_binding,
    read_record, remove_private_object, rename_noreplace, sync_directory, validate_link_count,
    validate_metadata, write_object_binding, write_record,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptPathKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResourceState {
    Planned,
    Prepared,
    Published,
    Quarantined,
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ObjectBinding {
    version: u8,
    attempt_token: String,
    object_token: String,
    identity: PersistentIdentity,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OwnedResource {
    canonical_path: PathBuf,
    temporary_path: PathBuf,
    quarantine_name: String,
    kind: AttemptPathKind,
    object_token: String,
    identity: Option<PersistentIdentity>,
    state: ResourceState,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AttemptRecord {
    version: u8,
    attempt_key: String,
    attempt_token: String,
    resources: Vec<OwnedResource>,
    #[serde(default)]
    quarantine: Option<OwnedResource>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttemptFaultPoint {
    PlanDurable,
    ObjectDurable,
    PreparedJournalStored,
    PublishedObjectDurable,
    PublishedJournalStored,
    QuarantinePlanDurable,
    QuarantineObjectStored,
    QuarantinePublished,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CleanupTestAction {
    CrashAfterMove,
    ReplaceQuarantineBeforeDelete,
}

pub(crate) struct TenantAttemptJournal {
    control_root: PathBuf,
    quarantine_root: PathBuf,
    journal_path: PathBuf,
    allowed_roots: Vec<PathBuf>,
    record: AttemptRecord,
    #[cfg(test)]
    fault: Option<AttemptFaultPoint>,
    #[cfg(test)]
    cleanup_actions: std::collections::HashMap<PathBuf, CleanupTestAction>,
}

impl TenantAttemptJournal {
    pub(crate) async fn begin(
        control_root: PathBuf,
        quarantine_root: PathBuf,
        attempt_key: &str,
        allowed_roots: Vec<PathBuf>,
    ) -> anyhow::Result<Self> {
        validate_attempt_key(attempt_key)?;
        let control_root = canonical_real_directory(&control_root)?;
        let quarantine_root = canonical_real_directory(&quarantine_root)?;
        let allowed_roots = allowed_roots
            .iter()
            .map(|path| canonical_real_directory(path))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let journal_path = control_root.join(format!("{attempt_key}.attempt.json"));
        if journal_path.exists() {
            let record = read_record(&journal_path)?;
            anyhow::ensure!(
                record.attempt_key == attempt_key,
                "tenant attempt journal key changed"
            );
            let mut stale = Self::from_record(
                control_root.clone(),
                quarantine_root.clone(),
                journal_path.clone(),
                allowed_roots.clone(),
                record,
            )?;
            stale.cleanup().await?;
        }
        let record = AttemptRecord {
            version: 2,
            attempt_key: attempt_key.to_string(),
            attempt_token: uuid::Uuid::new_v4().to_string(),
            resources: Vec::new(),
            quarantine: None,
        };
        write_record(&journal_path, &record)?;
        Self::from_record(
            control_root,
            quarantine_root,
            journal_path,
            allowed_roots,
            record,
        )
    }

    fn from_record(
        control_root: PathBuf,
        quarantine_root: PathBuf,
        journal_path: PathBuf,
        allowed_roots: Vec<PathBuf>,
        record: AttemptRecord,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            record.version == 2,
            "unsupported tenant attempt journal version"
        );
        uuid::Uuid::parse_str(&record.attempt_token).context("invalid tenant attempt token")?;
        for resource in &record.resources {
            validate_owned_path(&resource.canonical_path, &allowed_roots)?;
            validate_owned_path(&resource.temporary_path, &allowed_roots)?;
        }
        if let Some(resource) = &record.quarantine {
            validate_quarantine_resource(resource, &quarantine_root)?;
        }
        Ok(Self {
            control_root,
            quarantine_root,
            journal_path,
            allowed_roots,
            record,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            cleanup_actions: std::collections::HashMap::new(),
        })
    }

    pub(crate) async fn ensure_directory(&mut self, path: &Path) -> anyhow::Result<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                validate_metadata(&metadata, AttemptPathKind::Directory)?;
                Ok(false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.create_resource(path, AttemptPathKind::Directory)?;
                Ok(true)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn create_owned_file(&mut self, path: &Path) -> anyhow::Result<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                validate_metadata(&metadata, AttemptPathKind::File)?;
                Ok(false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.create_resource(path, AttemptPathKind::File)?;
                Ok(true)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn create_resource(&mut self, path: &Path, kind: AttemptPathKind) -> anyhow::Result<()> {
        let parent =
            canonical_real_directory(path.parent().context("tenant attempt path has no parent")?)?;
        let canonical_path = parent.join(
            path.file_name()
                .context("tenant attempt path has no file name")?,
        );
        validate_owned_path(&canonical_path, &self.allowed_roots)?;
        anyhow::ensure!(
            std::fs::canonicalize(path.parent().context("tenant attempt path has no parent")?)?
                == parent,
            "tenant attempt parent changed"
        );
        let object_token = uuid::Uuid::new_v4().to_string();
        let name = canonical_path
            .file_name()
            .context("tenant attempt path has no file name")?
            .to_string_lossy();
        let temporary_path = parent.join(format!(".{name}.ga-attempt-{object_token}.tmp"));
        let quarantine_name = format!("{object_token}.owned");
        let index = self.record.resources.len();
        self.record.resources.push(OwnedResource {
            canonical_path: canonical_path.clone(),
            temporary_path: temporary_path.clone(),
            quarantine_name,
            kind,
            object_token: object_token.clone(),
            identity: None,
            state: ResourceState::Planned,
        });
        self.persist()?;
        self.fault(AttemptFaultPoint::PlanDurable)?;

        create_private_object(&temporary_path, kind)?;
        let descriptor = open_nofollow(&temporary_path, kind, true)?;
        let identity = PersistentIdentity::from_file(&descriptor, kind)?;
        let binding = ObjectBinding {
            version: 1,
            attempt_token: self.record.attempt_token.clone(),
            object_token,
            identity: identity.clone(),
        };
        write_object_binding(&descriptor, &temporary_path, &binding, false)?;
        descriptor.sync_all()?;
        sync_directory(&parent)?;
        self.fault(AttemptFaultPoint::ObjectDurable)?;

        self.record.resources[index].identity = Some(identity);
        self.record.resources[index].state = ResourceState::Prepared;
        self.persist()?;
        self.fault(AttemptFaultPoint::PreparedJournalStored)?;

        rename_noreplace(&temporary_path, &canonical_path)?;
        sync_directory(&parent)?;
        self.fault(AttemptFaultPoint::PublishedObjectDurable)?;

        self.record.resources[index].state = ResourceState::Published;
        self.persist()?;
        self.fault(AttemptFaultPoint::PublishedJournalStored)?;
        Ok(())
    }

    pub(crate) async fn cleanup(&mut self) -> anyhow::Result<()> {
        let mut failures = Vec::new();
        let mut failed_paths = Vec::new();
        let mut removed_tokens = std::collections::HashSet::new();
        let mut order = (0..self.record.resources.len()).collect::<Vec<_>>();
        order.sort_by_key(|index| {
            std::cmp::Reverse(
                self.record.resources[*index]
                    .canonical_path
                    .components()
                    .count(),
            )
        });
        for index in order {
            let resource = self.record.resources[index].clone();
            if failed_paths
                .iter()
                .any(|failed: &PathBuf| failed.starts_with(&resource.canonical_path))
            {
                failures.push(anyhow::anyhow!(
                    "tenant directory retained because child cleanup failed"
                ));
                failed_paths.push(resource.canonical_path);
                continue;
            }
            if let Err(error) = self.cleanup_resource(&resource) {
                failures.push(error);
                failed_paths.push(resource.canonical_path);
                continue;
            }
            removed_tokens.insert(resource.object_token);
        }
        self.record
            .resources
            .retain(|resource| !removed_tokens.contains(&resource.object_token));
        self.persist()
            .context("tenant cleanup journal update failed")?;
        if !failures.is_empty() {
            anyhow::bail!(
                "tenant cleanup retained {} ownership record(s)",
                failures.len()
            );
        }
        self.remove_quarantine_if_empty()?;
        self.remove_journal()?;
        Ok(())
    }

    fn cleanup_resource(&mut self, resource: &OwnedResource) -> anyhow::Result<()> {
        let quarantine = self.quarantine_destination(resource);
        let source = self.locate_resource(resource, &quarantine)?;
        let Some(source) = source else {
            return Ok(());
        };
        let (opened, binding) = self.validate_bound_object(&source, resource)?;
        if source != quarantine {
            let quarantine_dir = self.ensure_quarantine_directory()?;
            rename_noreplace(&source, &quarantine)?;
            sync_directory(source.parent().context("tenant source has no parent")?)?;
            sync_directory(&quarantine_dir)?;
            #[cfg(test)]
            if self.cleanup_action(resource) == Some(CleanupTestAction::CrashAfterMove) {
                anyhow::bail!("injected crash after quarantine move");
            }
            let index = self
                .record
                .resources
                .iter()
                .position(|candidate| candidate.object_token == resource.object_token)
                .context("tenant cleanup resource disappeared")?;
            self.record.resources[index].state = ResourceState::Quarantined;
            self.record.resources[index].identity = Some(binding.identity.clone());
            self.persist()?;
        }
        let (moved, _) = self.validate_bound_object(&quarantine, resource)?;
        #[cfg(test)]
        if self.cleanup_action(resource) == Some(CleanupTestAction::ReplaceQuarantineBeforeDelete) {
            replace_quarantine_for_test(&quarantine, resource.kind)?;
        }
        let (current, _) = self.validate_bound_object(&quarantine, resource)?;
        anyhow::ensure!(
            PersistentIdentity::from_file(&moved, resource.kind)?
                == PersistentIdentity::from_file(&current, resource.kind)?,
            "tenant quarantine identity changed before deletion"
        );
        drop(opened);
        remove_private_object(&quarantine, resource.kind, &current)?;
        sync_directory(
            quarantine
                .parent()
                .context("tenant quarantine has no parent")?,
        )?;
        Ok(())
    }

    fn locate_resource(
        &self,
        resource: &OwnedResource,
        quarantine: &Path,
    ) -> anyhow::Result<Option<PathBuf>> {
        let candidates = match resource.state {
            ResourceState::Quarantined => [
                quarantine,
                &resource.canonical_path,
                &resource.temporary_path,
            ],
            ResourceState::Published => [
                &resource.canonical_path,
                quarantine,
                &resource.temporary_path,
            ],
            ResourceState::Prepared | ResourceState::Planned => [
                &resource.temporary_path,
                &resource.canonical_path,
                quarantine,
            ],
        };
        for candidate in candidates {
            match std::fs::symlink_metadata(candidate) {
                Ok(_) => return Ok(Some(candidate.to_path_buf())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }

    fn validate_bound_object(
        &self,
        path: &Path,
        resource: &OwnedResource,
    ) -> anyhow::Result<(File, ObjectBinding)> {
        let descriptor = open_nofollow(path, resource.kind, false)?;
        let actual = PersistentIdentity::from_file(&descriptor, resource.kind)?;
        let binding = read_object_binding(&descriptor, path)?;
        anyhow::ensure!(binding.version == 1, "unsupported tenant object binding");
        anyhow::ensure!(
            binding.attempt_token == self.record.attempt_token
                && binding.object_token == resource.object_token
                && binding.identity == actual,
            "tenant object ownership token or identity changed"
        );
        if let Some(expected) = &resource.identity {
            anyhow::ensure!(*expected == actual, "tenant journal identity changed");
        }
        Ok((descriptor, binding))
    }

    fn ensure_quarantine_directory(&mut self) -> anyhow::Result<PathBuf> {
        let path = self.quarantine_directory();
        if self.record.quarantine.is_none() {
            let object_token = uuid::Uuid::new_v4().to_string();
            self.record.quarantine = Some(OwnedResource {
                canonical_path: path.clone(),
                temporary_path: self
                    .quarantine_root
                    .join(format!(".{}.{}.tmp", self.record.attempt_key, object_token)),
                quarantine_name: String::new(),
                kind: AttemptPathKind::Directory,
                object_token,
                identity: None,
                state: ResourceState::Planned,
            });
            self.persist()?;
            self.fault(AttemptFaultPoint::QuarantinePlanDurable)?;
        }
        let mut resource = self
            .record
            .quarantine
            .clone()
            .context("tenant quarantine ownership record missing")?;
        if path.exists() {
            let (_, binding) = self.validate_bound_object(&path, &resource)?;
            resource.identity = Some(binding.identity);
            resource.state = ResourceState::Published;
            self.record.quarantine = Some(resource);
            self.persist()?;
            return Ok(path);
        }
        if resource.temporary_path.exists() {
            let (_, binding) = self.validate_bound_object(&resource.temporary_path, &resource)?;
            resource.identity = Some(binding.identity);
        } else {
            anyhow::ensure!(
                resource.identity.is_none() && resource.state == ResourceState::Planned,
                "tenant quarantine object disappeared after preparation"
            );
            create_private_object(&resource.temporary_path, AttemptPathKind::Directory)?;
            let descriptor =
                open_nofollow(&resource.temporary_path, AttemptPathKind::Directory, true)?;
            let identity = PersistentIdentity::from_file(&descriptor, AttemptPathKind::Directory)?;
            let binding = ObjectBinding {
                version: 1,
                attempt_token: self.record.attempt_token.clone(),
                object_token: resource.object_token.clone(),
                identity: identity.clone(),
            };
            write_object_binding(&descriptor, &resource.temporary_path, &binding, false)?;
            descriptor.sync_all()?;
            sync_directory(&self.quarantine_root)?;
            resource.identity = Some(identity);
            self.fault(AttemptFaultPoint::QuarantineObjectStored)?;
        }
        resource.state = ResourceState::Prepared;
        self.record.quarantine = Some(resource.clone());
        self.persist()?;
        rename_noreplace(&resource.temporary_path, &path)?;
        sync_directory(&self.quarantine_root)?;
        self.fault(AttemptFaultPoint::QuarantinePublished)?;
        resource.state = ResourceState::Published;
        self.record.quarantine = Some(resource);
        self.persist()?;
        Ok(path)
    }

    fn remove_quarantine_if_empty(&mut self) -> anyhow::Result<()> {
        let path = self.quarantine_directory();
        if !path.exists() {
            self.record.quarantine = None;
            self.persist()?;
            return Ok(());
        }
        let resource = self
            .record
            .quarantine
            .clone()
            .context("tenant quarantine ownership record missing")?;
        let (descriptor, _) = self.validate_bound_object(&path, &resource)?;
        anyhow::ensure!(
            std::fs::read_dir(&path)?.next().is_none(),
            "tenant quarantine directory is not empty"
        );
        remove_private_object(&path, AttemptPathKind::Directory, &descriptor)?;
        sync_directory(&self.quarantine_root)?;
        self.record.quarantine = None;
        self.persist()?;
        Ok(())
    }

    pub(crate) async fn commit(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.record
                .resources
                .iter()
                .all(|resource| resource.state == ResourceState::Published),
            "tenant attempt cannot commit unpublished resources"
        );
        anyhow::ensure!(
            self.record.quarantine.is_none(),
            "tenant attempt cannot commit cleanup quarantine"
        );
        self.remove_journal()
    }

    fn remove_journal(&self) -> anyhow::Result<()> {
        let current = read_record(&self.journal_path)?;
        anyhow::ensure!(
            current.attempt_token == self.record.attempt_token,
            "tenant attempt journal token changed"
        );
        let retired = self.control_root.join(format!(
            ".{}.{}.committed",
            self.record.attempt_key, self.record.attempt_token
        ));
        rename_noreplace(&self.journal_path, &retired)?;
        sync_directory(&self.control_root)?;
        let moved = read_record(&retired)?;
        anyhow::ensure!(
            moved.attempt_token == self.record.attempt_token,
            "retired tenant journal token changed"
        );
        std::fs::remove_file(&retired)?;
        sync_directory(&self.control_root)?;
        Ok(())
    }

    fn persist(&self) -> anyhow::Result<()> {
        write_record(&self.journal_path, &self.record)
    }

    fn quarantine_directory(&self) -> PathBuf {
        self.quarantine_root.join(format!(
            "{}-{}",
            self.record.attempt_key, self.record.attempt_token
        ))
    }

    fn quarantine_destination(&self, resource: &OwnedResource) -> PathBuf {
        self.quarantine_directory().join(&resource.quarantine_name)
    }

    #[cfg(test)]
    fn fault(&mut self, point: AttemptFaultPoint) -> anyhow::Result<()> {
        if self.fault == Some(point) {
            self.fault = None;
            anyhow::bail!("injected tenant attempt crash at {point:?}");
        }
        Ok(())
    }

    #[cfg(not(test))]
    fn fault(&mut self, _point: AttemptFaultPoint) -> anyhow::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    fn cleanup_action(&self, resource: &OwnedResource) -> Option<CleanupTestAction> {
        self.cleanup_actions.get(&resource.canonical_path).copied()
    }

    #[cfg(test)]
    pub(crate) fn fail_once_at_for_test(&mut self, point: AttemptFaultPoint) {
        self.fault = Some(point);
    }

    #[cfg(test)]
    pub(crate) fn set_cleanup_action_for_test(&mut self, path: PathBuf, action: CleanupTestAction) {
        self.cleanup_actions.insert(path, action);
    }

    #[cfg(test)]
    pub(crate) async fn replace_object_token_for_test(
        &self,
        path: &Path,
        token: &str,
    ) -> anyhow::Result<()> {
        let resource = self
            .record
            .resources
            .iter()
            .find(|resource| resource.canonical_path == path)
            .context("test resource missing")?;
        let descriptor = open_nofollow(path, resource.kind, true)?;
        let mut binding = read_object_binding(&descriptor, path)?;
        binding.object_token = token.to_string();
        write_object_binding(&descriptor, path, &binding, true)?;
        descriptor.sync_all()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn occupy_quarantine_destination_for_test(
        &mut self,
        path: &Path,
    ) -> anyhow::Result<()> {
        let quarantine_name = self.resource_for_test(path)?.quarantine_name.clone();
        let directory = self.ensure_quarantine_directory()?;
        std::fs::write(directory.join(quarantine_name), b"occupied")?;
        sync_directory(&directory)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn quarantine_destination_for_test(&self, path: &Path) -> anyhow::Result<PathBuf> {
        Ok(self.quarantine_destination(self.resource_for_test(path)?))
    }

    #[cfg(test)]
    fn resource_for_test(&self, path: &Path) -> anyhow::Result<&OwnedResource> {
        self.record
            .resources
            .iter()
            .find(|resource| resource.canonical_path == path)
            .context("test resource missing")
    }

    #[cfg(test)]
    pub(crate) fn resource_paths_for_test(&self) -> Vec<PathBuf> {
        self.record
            .resources
            .iter()
            .map(|resource| resource.canonical_path.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn journal_exists_for_current_attempt_for_test(&self) -> bool {
        self.journal_path.exists()
    }

    #[cfg(test)]
    pub(crate) fn journal_exists_for_test(control: &Path, key: &str) -> bool {
        control.join(format!("{key}.attempt.json")).exists()
    }
}

fn validate_attempt_key(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "tenant attempt key must be canonical lowercase ASCII"
    );
    Ok(())
}

fn validate_owned_path(path: &Path, roots: &[PathBuf]) -> anyhow::Result<()> {
    anyhow::ensure!(path.is_absolute(), "tenant attempt path must be absolute");
    anyhow::ensure!(
        roots
            .iter()
            .any(|root| path.starts_with(root) && path != root),
        "tenant attempt path escaped configured roots"
    );
    anyhow::ensure!(
        path.components()
            .all(|component| !matches!(component, std::path::Component::ParentDir)),
        "tenant attempt path contains traversal"
    );
    Ok(())
}

fn validate_quarantine_resource(
    resource: &OwnedResource,
    quarantine_root: &Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        resource.kind == AttemptPathKind::Directory
            && resource.canonical_path.parent() == Some(quarantine_root)
            && resource.temporary_path.parent() == Some(quarantine_root),
        "tenant quarantine journal path escaped configured root"
    );
    Ok(())
}
