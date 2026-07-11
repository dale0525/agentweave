use crate::skill::{SkillManifest, manifest_entry_resources};
use crate::skill_package::SkillPackageId;
use crate::skill_source::canonical_relative_path;
use crate::skill_state::{SkillInstallStatus, SkillLayerRecord, SkillRevisionStatus};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_faults::StoreFaultPoint;
use crate::skill_store_fs::open_regular_file_nofollow;
use crate::skill_store_fs::{PackageLimits, copy_prepared_package_tree_into_reserved};
use crate::skill_store_locks::acquire_revision_lock;
use crate::skill_store_operations::{ensure_exact_path, error_is_not_found};
use crate::skill_store_secure_fs::secure_package_hash;
use crate::skill_store_secure_roots::open_prepared_directory;
use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) struct PreparedSkillExecution {
    command: String,
    args: Vec<String>,
    current_dir: PathBuf,
    _temporary: tempfile::TempDir,
}

impl PreparedSkillExecution {
    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    pub(crate) fn current_dir(&self) -> &Path {
        &self.current_dir
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ExecutionCommandKind {
    Bare,
    PackagedRelative(PathBuf),
    Absolute,
}

#[derive(Debug, Eq, PartialEq)]
struct NormalizedAbsolutePath {
    prefix: String,
    components: Vec<String>,
}

pub(crate) fn classify_execution_command(
    command: &str,
    windows: bool,
) -> anyhow::Result<ExecutionCommandKind> {
    if command.is_empty() || command.contains('\0') {
        anyhow::bail!("invalid empty or nul execution command");
    }
    if is_absolute_path(command, windows) {
        return Ok(ExecutionCommandKind::Absolute);
    }
    if windows && has_windows_anchored_prefix(command) {
        anyhow::bail!("execution command is not package-relative: {command}");
    }
    let has_separator = command.contains('/') || command.contains('\\');
    if !has_separator {
        if matches!(command, "." | "..") {
            anyhow::bail!("execution command is not a bare executable name: {command}");
        }
        return Ok(ExecutionCommandKind::Bare);
    }

    let mut relative = PathBuf::new();
    for component in command.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => anyhow::bail!("unsafe packaged execution command: {command}"),
            component => relative.push(component),
        }
    }
    if relative.as_os_str().is_empty() {
        anyhow::bail!("execution command has no packaged path: {command}");
    }
    canonical_relative_path(&relative)?;
    Ok(ExecutionCommandKind::PackagedRelative(relative))
}

pub(crate) fn is_portable_absolute_path(value: &str) -> bool {
    is_absolute_path(value, false) || is_absolute_path(value, true)
}

fn is_absolute_path(value: &str, windows: bool) -> bool {
    normalize_absolute_path(value, windows).is_some()
}

pub(crate) fn absolute_execution_command_is_within(
    command: &str,
    root: &str,
    windows: bool,
) -> bool {
    let Some(command) = normalize_absolute_path(command, windows) else {
        return false;
    };
    let Some(root) = normalize_absolute_path(root, windows) else {
        return false;
    };
    command.prefix == root.prefix
        && command.components.len() >= root.components.len()
        && command.components[..root.components.len()] == root.components
}

fn has_windows_anchored_prefix(command: &str) -> bool {
    let bytes = command.as_bytes();
    command.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn normalize_absolute_path(value: &str, windows: bool) -> Option<NormalizedAbsolutePath> {
    if windows {
        normalize_windows_absolute_path(value)
    } else {
        let remainder = value.strip_prefix('/')?;
        Some(NormalizedAbsolutePath {
            prefix: "/".into(),
            components: normalize_components(remainder.split('/'), false),
        })
    }
}

fn normalize_windows_absolute_path(value: &str) -> Option<NormalizedAbsolutePath> {
    let mut value = value.replace('/', "\\").to_lowercase();
    if let Some(remainder) = value.strip_prefix(r"\\?\unc\") {
        value = format!(r"\\{remainder}");
    } else if let Some(remainder) = value.strip_prefix(r"\\?\") {
        value = remainder.to_string();
    }
    let bytes = value.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return Some(NormalizedAbsolutePath {
            prefix: value[..2].to_string(),
            components: normalize_components(value[3..].split('\\'), true),
        });
    }
    let remainder = value.strip_prefix(r"\\")?;
    let mut components = remainder
        .split('\\')
        .filter(|component| !component.is_empty());
    let server = components.next()?;
    let share = components.next()?;
    Some(NormalizedAbsolutePath {
        prefix: format!(r"\\{server}\{share}"),
        components: normalize_components(components, true),
    })
}

fn normalize_components<'a>(
    components: impl IntoIterator<Item = &'a str>,
    case_insensitive: bool,
) -> Vec<String> {
    let mut normalized = Vec::new();
    for component in components {
        match component {
            "" | "." => {}
            ".." => {
                normalized.pop();
            }
            component if case_insensitive => normalized.push(component.to_lowercase()),
            component => normalized.push(component.to_string()),
        }
    }
    normalized
}

impl SkillRevisionStore {
    pub(crate) async fn prepare_managed_execution(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        expected_path: &Path,
        expected_hash: &str,
        limits: PackageLimits,
        manifest: &SkillManifest,
    ) -> anyhow::Result<PreparedSkillExecution> {
        let guard = acquire_revision_lock(&self.paths.identity, revision_id, &self.faults).await?;
        self.paths.verify_identity()?;
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .with_context(|| format!("managed execution revision not found: {revision_id}"))?;
        let installation = self
            .state
            .get_installation(package_id)
            .await?
            .with_context(|| {
                format!(
                    "managed execution installation not found: {}",
                    package_id.as_str()
                )
            })?;
        if record.status != SkillRevisionStatus::Managed
            || &record.package_id != package_id
            || record.content_hash != expected_hash
            || installation.source_layer != SkillLayerRecord::Managed
            || installation.status != SkillInstallStatus::Active
            || !installation.enabled
            || installation.active_revision_id.as_deref() != Some(revision_id)
        {
            anyhow::bail!("no longer active managed revision: {revision_id}");
        }
        ensure_exact_path(
            Path::new(&record.storage_path),
            expected_path,
            "managed execution",
        )?;
        let relative = PathBuf::from(package_id.as_str())
            .join("revisions")
            .join(revision_id);
        let managed_directory =
            open_prepared_directory(self.paths.managed_identity(), &relative).await?;
        let temporary = tempfile::Builder::new()
            .prefix("general-agent-skill-execution-")
            .tempdir()?;
        copy_prepared_package_tree_into_reserved(
            &managed_directory,
            temporary.path(),
            limits,
            &self.faults,
            StoreFaultPoint::ExecutionCopyFile,
        )
        .await?;
        let actual = secure_package_hash(temporary.path(), limits).await?;
        if actual != expected_hash {
            anyhow::bail!("managed execution snapshot hash mismatch: {revision_id}");
        }
        let command = prepare_execution_command(
            &manifest.entry.command,
            temporary.path(),
            &self.paths,
            expected_path,
        )
        .await?;
        for resource in manifest_entry_resources(manifest) {
            match open_regular_file_nofollow(temporary.path(), &resource).await {
                Ok(_) => {}
                Err(error) if error_is_not_found(&error) => anyhow::bail!(
                    "private execution entry resource does not exist: {}",
                    resource.display()
                ),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "private execution entry resource is not a contained regular file: {}",
                            resource.display()
                        )
                    });
                }
            }
        }
        drop(guard);
        self.faults
            .checkpoint(StoreFaultPoint::ExecutionAfterSnapshot)
            .await;
        let current_dir = temporary.path().to_path_buf();
        Ok(PreparedSkillExecution {
            command,
            // Arguments are process data, not a filesystem sandbox. Resource-shaped relative
            // arguments were validated above; every other argument remains opaque.
            args: manifest.entry.args.clone(),
            current_dir,
            _temporary: temporary,
        })
    }
}

async fn prepare_execution_command(
    command: &str,
    private_root: &Path,
    paths: &crate::skill_store::SkillStorePaths,
    authoritative_revision: &Path,
) -> anyhow::Result<String> {
    match classify_execution_command(command, cfg!(windows))? {
        ExecutionCommandKind::Bare => Ok(command.to_string()),
        ExecutionCommandKind::PackagedRelative(relative) => {
            match open_regular_file_nofollow(private_root, &relative).await {
                Ok(_) => {}
                Err(error) if error_is_not_found(&error) => anyhow::bail!(
                    "private execution command does not exist: {}",
                    relative.display()
                ),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "private execution command is not a contained regular file: {}",
                            relative.display()
                        )
                    });
                }
            }
            Ok(private_root.join(relative).to_string_lossy().into_owned())
        }
        ExecutionCommandKind::Absolute => {
            reject_absolute_managed_command(command, paths, authoritative_revision).await?;
            Ok(command.to_string())
        }
    }
}

async fn reject_absolute_managed_command(
    command: &str,
    paths: &crate::skill_store::SkillStorePaths,
    authoritative_revision: &Path,
) -> anyhow::Result<()> {
    let managed_roots = [authoritative_revision, paths.managed.as_path()];
    if managed_roots.iter().any(|root| {
        absolute_execution_command_is_within(command, &root.to_string_lossy(), cfg!(windows))
    }) {
        anyhow::bail!("absolute managed command bypasses private execution snapshot: {command}");
    }

    let resolved = match tokio::fs::canonicalize(command).await {
        Ok(resolved) => resolved,
        Err(_) => return Ok(()),
    };
    for root in managed_roots {
        let resolved_root = tokio::fs::canonicalize(root).await.with_context(|| {
            format!(
                "failed to resolve managed command boundary: {}",
                root.display()
            )
        })?;
        if absolute_execution_command_is_within(
            &resolved.to_string_lossy(),
            &resolved_root.to_string_lossy(),
            cfg!(windows),
        ) {
            anyhow::bail!(
                "absolute managed command bypasses private execution snapshot: {command}"
            );
        }
    }
    Ok(())
}
