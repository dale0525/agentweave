use crate::skill::{SkillManifest, manifest_entry_resources};
use crate::skill_package::SkillPackageId;
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

pub(crate) fn execution_text_references_path(value: &str, protected: &Path, windows: bool) -> bool {
    let protected = normalize_execution_text(&protected.to_string_lossy(), windows)
        .trim_end_matches(if windows { '\\' } else { '/' })
        .to_string();
    if protected.is_empty() {
        return false;
    }
    let Some(protected) = lexically_normalize_absolute(&protected, windows) else {
        return false;
    };

    text_references_normalized_path(value, &protected, windows)
        || decoded_file_uri_paths(value, windows)
            .iter()
            .any(|path| text_references_normalized_path(path, &protected, windows))
}

fn is_embedded_path_boundary(character: char) -> bool {
    !character.is_alphanumeric() && !matches!(character, '_' | '-' | '.' | '/' | '\\')
}

fn text_references_normalized_path(value: &str, protected: &str, windows: bool) -> bool {
    let separator = if windows { '\\' } else { '/' };
    absolute_path_candidates(value)
        .into_iter()
        .any(|candidate| {
            let candidate = normalize_execution_text(candidate, windows);
            lexically_normalize_absolute(&candidate, windows).is_some_and(|candidate| {
                candidate == protected
                    || candidate
                        .strip_prefix(protected)
                        .is_some_and(|suffix| suffix.starts_with(separator))
            })
        })
}

fn absolute_path_candidates(value: &str) -> Vec<&str> {
    let bytes = value.as_bytes();
    let mut candidates = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if is_absolute_path_candidate_start(value, index) {
            let end = value[index..]
                .char_indices()
                .find_map(|(offset, character)| {
                    (offset > 0
                        && is_path_candidate_terminator(character)
                        && !is_windows_extended_prefix_marker(value, index, offset, character))
                    .then_some(index + offset)
                })
                .unwrap_or(bytes.len());
            candidates.push(&value[index..end]);
            index = end;
        } else {
            index += value[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    candidates
}

fn is_windows_extended_prefix_marker(
    value: &str,
    start: usize,
    offset: usize,
    character: char,
) -> bool {
    let prefix = value.as_bytes().get(start..start + 2);
    character == '?' && offset == 2 && (prefix == Some(&b"\\\\"[..]) || prefix == Some(&b"//"[..]))
}

fn is_absolute_path_candidate_start(value: &str, index: usize) -> bool {
    let bytes = value.as_bytes();
    let boundary = value[..index]
        .chars()
        .next_back()
        .is_none_or(is_embedded_path_boundary);
    if !boundary {
        return false;
    }
    let remaining = &bytes[index..];
    if remaining.starts_with(b"\\\\") {
        return true;
    }
    if remaining.len() >= 3
        && remaining[0].is_ascii_alphabetic()
        && remaining[1] == b':'
        && matches!(remaining[2], b'/' | b'\\')
    {
        return true;
    }
    if remaining.first() != Some(&b'/') {
        return false;
    }
    let previous = bytes.get(index.wrapping_sub(1)).copied();
    let before_previous = bytes.get(index.wrapping_sub(2)).copied();
    let starts_uri_authority = previous == Some(b':') && remaining.get(1) == Some(&b'/');
    let belongs_to_drive =
        previous == Some(b':') && before_previous.is_some_and(|byte| byte.is_ascii_alphabetic());
    !starts_uri_authority && !belongs_to_drive
}

fn is_path_candidate_terminator(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '\'' | '"'
                | ','
                | ';'
                | '|'
                | '&'
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '?'
                | '#'
        )
}

fn decoded_file_uri_paths(value: &str, windows: bool) -> Vec<String> {
    let bytes = value.as_bytes();
    let mut paths = Vec::new();
    let mut index = 0;
    while index + 5 <= bytes.len() {
        let scheme = &bytes[index..index + 5];
        let boundary = value[..index]
            .chars()
            .next_back()
            .is_none_or(is_embedded_path_boundary);
        if boundary && scheme.eq_ignore_ascii_case(b"file:") {
            let payload_start = index + 5;
            let payload_end = value[payload_start..]
                .char_indices()
                .find_map(|(offset, character)| {
                    is_path_candidate_terminator(character).then_some(payload_start + offset)
                })
                .unwrap_or(bytes.len());
            if let Some(decoded) = percent_decode_utf8(&value[payload_start..payload_end]) {
                paths.push(normalize_file_uri_path(decoded, windows));
            }
            index = payload_end;
        } else {
            index += value[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    paths
}

fn percent_decode_utf8(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn normalize_file_uri_path(value: String, windows: bool) -> String {
    if !windows {
        return value;
    }
    let path = value.trim_start_matches(['/', '\\']);
    let bytes = path.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        return path.to_string();
    }
    if value.len() - path.len() >= 2 {
        format!(r"\\{path}")
    } else {
        value
    }
}

fn lexically_normalize_absolute(value: &str, windows: bool) -> Option<String> {
    let separator = if windows { '\\' } else { '/' };
    let (prefix, remainder) = if windows {
        let bytes = value.as_bytes();
        if bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'\\' {
            (&value[..2], &value[3..])
        } else if let Some(remainder) = value.strip_prefix("\\\\") {
            ("\\\\", remainder)
        } else {
            return None;
        }
    } else if let Some(remainder) = value.strip_prefix('/') {
        ("/", remainder)
    } else {
        return None;
    };
    let mut components = Vec::new();
    for component in remainder.split(separator) {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    if prefix == "/" || prefix == "\\\\" {
        Some(format!(
            "{prefix}{}",
            components.join(&separator.to_string())
        ))
    } else if components.is_empty() {
        Some(format!("{prefix}{separator}"))
    } else {
        Some(format!(
            "{prefix}{separator}{}",
            components.join(&separator.to_string())
        ))
    }
}

fn normalize_execution_text(value: &str, windows: bool) -> String {
    if windows {
        value
            .replace('/', "\\")
            .to_lowercase()
            .replace("\\\\?\\unc\\", "\\\\")
            .replace("\\\\?\\", "")
    } else {
        value.to_string()
    }
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
        review_execution_binding(manifest, &self.paths, expected_path)?;
        for resource in manifest_entry_resources(manifest) {
            match open_regular_file_nofollow(temporary.path(), resource).await {
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
            command: manifest.entry.command.clone(),
            args: manifest.entry.args.clone(),
            current_dir,
            _temporary: temporary,
        })
    }
}

fn review_execution_binding(
    manifest: &SkillManifest,
    paths: &crate::skill_store::SkillStorePaths,
    authoritative_revision: &Path,
) -> anyhow::Result<()> {
    let locks = paths.managed.join(".locks");
    let protected = [
        ("authoritative revision", authoritative_revision),
        ("locks root", locks.as_path()),
        ("managed root", paths.managed.as_path()),
        ("staging root", paths.staging.as_path()),
        ("quarantine root", paths.quarantine.as_path()),
    ];
    reject_execution_store_reference("command", &manifest.entry.command, &protected)?;
    for arg in &manifest.entry.args {
        reject_execution_store_reference("argument", arg, &protected)?;
    }
    Ok(())
}

fn reject_execution_store_reference(
    value_kind: &str,
    value: &str,
    protected: &[(&str, &Path)],
) -> anyhow::Result<()> {
    if let Some((root_kind, _)) = protected
        .iter()
        .find(|(_, path)| execution_text_references_path(value, path, cfg!(windows)))
    {
        anyhow::bail!("managed execution {value_kind} references skill store {root_kind}: {value}");
    }
    Ok(())
}
