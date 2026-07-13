use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

#[path = "tenant_attempt_fs.rs"]
mod fs;
use fs::{
    canonical_real_directory, clear_owned_directory_contents, create_private_object,
    object_binding_exists, open_delete_nofollow, open_nofollow, read_object_binding, read_record,
    remove_private_object, rename_noreplace, sync_directory, validate_metadata,
    validate_opened_link_count, write_object_binding, write_record,
};
#[cfg(test)]
use fs::{replace_quarantine_for_test, replace_temporary_source_for_test};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptPathKind {
    File,
    Directory,
}

#[cfg(any(windows, test))]
pub(crate) fn windows_link_count_is_one(link_count: Option<u32>) -> bool {
    link_count == Some(1)
}

#[cfg(windows)]
fn windows_file_information(
    file: &File,
) -> anyhow::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut information) };
    anyhow::ensure!(
        result != 0,
        "Windows tenant file information query failed: {}",
        std::io::Error::last_os_error()
    );
    Ok(information)
}

#[cfg(windows)]
pub(crate) fn windows_number_of_links(file: &File) -> anyhow::Result<Option<u32>> {
    Ok(Some(windows_file_information(file)?.nNumberOfLinks))
}

#[cfg(test)]
pub(crate) fn windows_open_contract_for_test() -> (u32, u32, u32, u32, u32) {
    let contract = fs::windows_open_contract_for_test();
    (
        contract.share_mode,
        contract.directory_flags,
        contract.directory_sync_access,
        contract.directory_write_access,
        contract.cleanup_access,
    )
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ResourceState {
    Planned,
    Creating,
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
        validate_opened_link_count(file, kind)?;
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
            let information = windows_file_information(file)?;
            Ok(Self {
                kind,
                volume: information.dwVolumeSerialNumber,
                file_index: (u64::from(information.nFileIndexHigh) << 32)
                    | u64::from(information.nFileIndexLow),
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
    ObjectCreatedBeforeBinding,
    ObjectDurable,
    PreparedJournalStored,
    PublishedObjectDurable,
    PublishedJournalStored,
    QuarantinePlanDurable,
    QuarantineCreatedBeforeBinding,
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
    #[cfg(test)]
    publish_replacements: std::collections::HashSet<PathBuf>,
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
            #[cfg(test)]
            publish_replacements: std::collections::HashSet::new(),
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

        ensure_path_absent(&temporary_path)?;
        self.record.resources[index].state = ResourceState::Creating;
        self.persist()?;
        create_private_object(&temporary_path, kind)?;
        self.fault(AttemptFaultPoint::ObjectCreatedBeforeBinding)?;
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
        drop(descriptor);

        self.record.resources[index].identity = Some(identity);
        self.record.resources[index].state = ResourceState::Prepared;
        self.persist()?;
        self.fault(AttemptFaultPoint::PreparedJournalStored)?;
        #[cfg(test)]
        if self.publish_replacements.remove(&canonical_path) {
            replace_temporary_source_for_test(&temporary_path, kind)?;
        }

        rename_noreplace(&temporary_path, &canonical_path)?;
        sync_directory(&parent)?;
        let published = self.record.resources[index].clone();
        self.validate_bound_object(&canonical_path, &published, false)?;
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
        let Some((source, opened, binding)) = source else {
            return Ok(());
        };
        let mut source_opened = Some(opened);
        if source != quarantine {
            let quarantine_dir = self.ensure_quarantine_directory()?;
            drop(source_opened.take());
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
        drop(source_opened.take());
        let (moved, _) = self.validate_bound_object(&quarantine, resource, true)?;
        if self.is_owned_unit_root(resource) {
            clear_owned_directory_contents(&moved, &quarantine)?;
        }
        let moved_identity = PersistentIdentity::from_file(&moved, resource.kind)?;
        drop(moved);
        #[cfg(test)]
        if self.cleanup_action(resource) == Some(CleanupTestAction::ReplaceQuarantineBeforeDelete) {
            replace_quarantine_for_test(&quarantine, resource.kind)?;
        }
        let (current, _) = self.validate_bound_object(&quarantine, resource, true)?;
        anyhow::ensure!(
            moved_identity == PersistentIdentity::from_file(&current, resource.kind)?,
            "tenant quarantine identity changed before deletion"
        );
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
    ) -> anyhow::Result<Option<(PathBuf, File, ObjectBinding)>> {
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
            ResourceState::Prepared | ResourceState::Creating | ResourceState::Planned => [
                &resource.temporary_path,
                &resource.canonical_path,
                quarantine,
            ],
        };
        let mut mismatches = 0_usize;
        for candidate in candidates {
            match std::fs::symlink_metadata(candidate) {
                Ok(_) => match self.validate_bound_object(candidate, resource, true) {
                    Ok((descriptor, binding)) => {
                        return Ok(Some((candidate.to_path_buf(), descriptor, binding)));
                    }
                    Err(_) if candidate == resource.temporary_path => {
                        if !self.remove_unbound_planned_temporary(resource)? {
                            mismatches += 1;
                        }
                    }
                    Err(_) => mismatches += 1,
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        anyhow::ensure!(
            mismatches == 0,
            "tenant owned resource candidates exist but none match the journal binding"
        );
        Ok(None)
    }

    fn remove_unbound_planned_temporary(&self, resource: &OwnedResource) -> anyhow::Result<bool> {
        if resource.state != ResourceState::Creating || resource.identity.is_some() {
            return Ok(false);
        }
        let descriptor = open_delete_nofollow(&resource.temporary_path, resource.kind)?;
        if object_binding_exists(&descriptor, &resource.temporary_path)? {
            return Ok(false);
        }
        remove_private_object(&resource.temporary_path, resource.kind, &descriptor)?;
        sync_directory(
            resource
                .temporary_path
                .parent()
                .context("tenant planned temporary has no parent")?,
        )?;
        Ok(true)
    }

    fn is_owned_unit_root(&self, resource: &OwnedResource) -> bool {
        resource.kind == AttemptPathKind::Directory
            && self
                .allowed_roots
                .iter()
                .any(|root| resource.canonical_path.parent() == Some(root.as_path()))
    }

    fn validate_bound_object(
        &self,
        path: &Path,
        resource: &OwnedResource,
        for_delete: bool,
    ) -> anyhow::Result<(File, ObjectBinding)> {
        let descriptor = if for_delete {
            open_delete_nofollow(path, resource.kind)?
        } else {
            open_nofollow(path, resource.kind, false)?
        };
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
            let (_, binding) = self.validate_bound_object(&path, &resource, false)?;
            resource.identity = Some(binding.identity);
            resource.state = ResourceState::Published;
            self.record.quarantine = Some(resource);
            self.persist()?;
            return Ok(path);
        }
        if resource.temporary_path.exists() {
            match self.validate_bound_object(&resource.temporary_path, &resource, false) {
                Ok((_, binding)) => resource.identity = Some(binding.identity),
                Err(error) => {
                    if !self.remove_unbound_planned_temporary(&resource)? {
                        return Err(error);
                    }
                }
            }
        }
        if !resource.temporary_path.exists() {
            anyhow::ensure!(
                resource.identity.is_none()
                    && matches!(
                        resource.state,
                        ResourceState::Planned | ResourceState::Creating
                    ),
                "tenant quarantine object disappeared after preparation"
            );
            if resource.state == ResourceState::Planned {
                ensure_path_absent(&resource.temporary_path)?;
                resource.state = ResourceState::Creating;
                self.record.quarantine = Some(resource.clone());
                self.persist()?;
            }
            create_private_object(&resource.temporary_path, AttemptPathKind::Directory)?;
            self.fault(AttemptFaultPoint::QuarantineCreatedBeforeBinding)?;
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
            drop(descriptor);
        }
        resource.state = ResourceState::Prepared;
        self.record.quarantine = Some(resource.clone());
        self.persist()?;
        rename_noreplace(&resource.temporary_path, &path)?;
        sync_directory(&self.quarantine_root)?;
        self.validate_bound_object(&path, &resource, false)?;
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
        let (descriptor, _) = self.validate_bound_object(&path, &resource, true)?;
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
    pub(crate) fn replace_temporary_source_for_test(&mut self, path: PathBuf) {
        self.publish_replacements.insert(path);
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
    pub(crate) fn temporary_path_for_test(&self, path: &Path) -> anyhow::Result<PathBuf> {
        Ok(self.resource_for_test(path)?.temporary_path.clone())
    }

    #[cfg(test)]
    pub(crate) fn quarantine_temporary_path_for_test(&self) -> anyhow::Result<PathBuf> {
        self.record
            .quarantine
            .as_ref()
            .map(|resource| resource.temporary_path.clone())
            .context("test quarantine resource missing")
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

fn ensure_path_absent(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => anyhow::bail!("tenant planned temporary path is already occupied"),
    }
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
