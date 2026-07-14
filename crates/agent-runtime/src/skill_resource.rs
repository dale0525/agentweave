use crate::skill_package::SkillPackageId;
use crate::skill_snapshot::SkillSnapshotLease;
use crate::skill_source::canonical_relative_path;
use crate::skill_store::SkillStoreLimits;
use crate::skill_store_fs::open_regular_file_nofollow;
use crate::skill_store_secure_fs::secure_package_hash;
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

pub const DEFAULT_MAX_REFERENCE_BYTES: u64 = 1024 * 1024;
pub const DEFAULT_MAX_REFERENCE_CHARS: usize = 512 * 1024;
pub const DEFAULT_MAX_SCRIPT_BYTES: u64 = 2 * 1024 * 1024;
pub const DEFAULT_MAX_SCRIPT_CHARS: usize = 1024 * 1024;
pub const DEFAULT_MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_RESOURCE_PATH_BYTES: u64 = 1024;
pub const DEFAULT_MAX_MEDIA_HEADER_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_IMAGE_DIMENSION: u32 = 16_384;
pub const DEFAULT_MAX_IMAGE_PIXELS: u64 = 100_000_000;

const MAX_REVISION_LABEL_BYTES: usize = 256;
const MAX_CONTENT_HASH_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillResourceKind {
    Reference,
    Script,
    Asset,
}

impl SkillResourceKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Reference => "references",
            Self::Script => "scripts",
            Self::Asset => "assets",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillResourcePath {
    relative: PathBuf,
    canonical: String,
}

impl SkillResourcePath {
    pub fn parse(value: &str) -> Result<Self, SkillResourceError> {
        if value.is_empty()
            || value.contains('\0')
            || value.contains('\\')
            || value
                .split('/')
                .any(|component| matches!(component, "" | "." | ".."))
        {
            return Err(SkillResourceError::InvalidPath(value.to_string()));
        }
        let relative = PathBuf::from(value);
        let canonical = canonical_relative_path(&relative)
            .map_err(|_| SkillResourceError::InvalidPath(value.to_string()))?;
        let canonical = String::from_utf8(canonical)
            .map_err(|_| SkillResourceError::InvalidPath(value.to_string()))?;
        Ok(Self {
            relative,
            canonical,
        })
    }

    pub fn as_path(&self) -> &Path {
        &self.relative
    }

    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    fn validate_kind(&self, kind: SkillResourceKind) -> Result<(), SkillResourceError> {
        let mut components = self.relative.components();
        let root = components
            .next()
            .and_then(|component| component.as_os_str().to_str());
        if root != Some(kind.directory()) || components.next().is_none() {
            return Err(SkillResourceError::KindPathMismatch {
                kind,
                path: self.canonical.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillResourceLimits {
    pub max_reference_bytes: u64,
    pub max_reference_chars: usize,
    pub max_script_bytes: u64,
    pub max_script_chars: usize,
    pub max_asset_bytes: u64,
    pub max_path_bytes: u64,
    pub max_media_header_bytes: usize,
    pub max_image_dimension: u32,
    pub max_image_pixels: u64,
}

impl Default for SkillResourceLimits {
    fn default() -> Self {
        Self {
            max_reference_bytes: DEFAULT_MAX_REFERENCE_BYTES,
            max_reference_chars: DEFAULT_MAX_REFERENCE_CHARS,
            max_script_bytes: DEFAULT_MAX_SCRIPT_BYTES,
            max_script_chars: DEFAULT_MAX_SCRIPT_CHARS,
            max_asset_bytes: DEFAULT_MAX_ASSET_BYTES,
            max_path_bytes: DEFAULT_MAX_RESOURCE_PATH_BYTES,
            max_media_header_bytes: DEFAULT_MAX_MEDIA_HEADER_BYTES,
            max_image_dimension: DEFAULT_MAX_IMAGE_DIMENSION,
            max_image_pixels: DEFAULT_MAX_IMAGE_PIXELS,
        }
    }
}

impl SkillResourceLimits {
    fn validate(self) -> Result<Self, SkillResourceError> {
        if self.max_reference_bytes == 0
            || self.max_reference_chars == 0
            || self.max_script_bytes == 0
            || self.max_script_chars == 0
            || self.max_asset_bytes == 0
            || self.max_path_bytes == 0
            || self.max_media_header_bytes == 0
            || self.max_image_dimension == 0
            || self.max_image_pixels == 0
        {
            return Err(SkillResourceError::InvalidLimits);
        }
        Ok(self)
    }

    fn byte_limit(self, kind: SkillResourceKind) -> u64 {
        match kind {
            SkillResourceKind::Reference => self.max_reference_bytes,
            SkillResourceKind::Script => self.max_script_bytes,
            SkillResourceKind::Asset => self.max_asset_bytes,
        }
    }

    fn char_limit(self, kind: SkillResourceKind) -> Option<usize> {
        match kind {
            SkillResourceKind::Reference => Some(self.max_reference_chars),
            SkillResourceKind::Script => Some(self.max_script_chars),
            SkillResourceKind::Asset => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillResourceRevision {
    snapshot_generation: u64,
    package_id: SkillPackageId,
    revision_id: String,
    package_root: PathBuf,
    expected_content_hash: String,
    known_files: BTreeSet<String>,
    package_limits: SkillStoreLimits,
    lease: Option<SkillSnapshotLease>,
    lease_revision_id: Option<String>,
}

impl SkillResourceRevision {
    #[allow(clippy::too_many_arguments)]
    pub fn from_verified_revision(
        snapshot_generation: u64,
        package_id: SkillPackageId,
        revision_id: impl Into<String>,
        package_root: impl Into<PathBuf>,
        expected_content_hash: impl Into<String>,
        known_files: impl IntoIterator<Item = PathBuf>,
        package_limits: SkillStoreLimits,
    ) -> Result<Self, SkillResourceError> {
        Self::build(
            snapshot_generation,
            package_id,
            revision_id.into(),
            package_root.into(),
            expected_content_hash.into(),
            known_files,
            package_limits,
            None,
            None,
        )
    }

    pub fn from_snapshot_lease(
        lease: &SkillSnapshotLease,
        package_id: &SkillPackageId,
    ) -> Result<Self, SkillResourceError> {
        let resolved = lease
            .snapshot()
            .packages()
            .iter()
            .find(|resolved| &resolved.package.descriptor.id == package_id)
            .ok_or_else(|| SkillResourceError::PackageNotInSnapshot(package_id.clone()))?;
        let verified = resolved
            .package
            .verified_content
            .as_ref()
            .ok_or_else(|| SkillResourceError::UnverifiedPackage(package_id.clone()))?;
        let managed_revision = verified
            .execution_binding
            .as_ref()
            .map(|binding| binding.revision_id.clone());
        let revision_id = managed_revision
            .clone()
            .unwrap_or_else(|| format!("content:{}", verified.expected_content_hash));
        Self::build(
            lease.generation(),
            package_id.clone(),
            revision_id,
            resolved.package.root.clone(),
            verified.expected_content_hash.clone(),
            verified.file_paths.iter().cloned(),
            verified.limits,
            Some(lease.clone()),
            managed_revision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        snapshot_generation: u64,
        package_id: SkillPackageId,
        revision_id: String,
        package_root: PathBuf,
        expected_content_hash: String,
        known_files: impl IntoIterator<Item = PathBuf>,
        package_limits: SkillStoreLimits,
        lease: Option<SkillSnapshotLease>,
        lease_revision_id: Option<String>,
    ) -> Result<Self, SkillResourceError> {
        validate_bounded_label(&revision_id, MAX_REVISION_LABEL_BYTES, "revision id")?;
        validate_bounded_label(
            &expected_content_hash,
            MAX_CONTENT_HASH_BYTES,
            "content hash",
        )?;
        if !package_root.is_absolute() {
            return Err(SkillResourceError::InvalidBinding(
                "package root must be absolute".into(),
            ));
        }
        let known_files = known_files
            .into_iter()
            .map(|path| {
                canonical_relative_path(&path)
                    .and_then(|bytes| String::from_utf8(bytes).map_err(Into::into))
                    .map_err(|_| {
                        SkillResourceError::InvalidBinding(format!(
                            "known resource path is not portable: {}",
                            path.display()
                        ))
                    })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self {
            snapshot_generation,
            package_id,
            revision_id,
            package_root,
            expected_content_hash,
            known_files,
            package_limits,
            lease,
            lease_revision_id,
        })
    }

    pub fn snapshot_generation(&self) -> u64 {
        self.snapshot_generation
    }

    pub fn package_id(&self) -> &SkillPackageId {
        &self.package_id
    }

    pub fn revision_id(&self) -> &str {
        &self.revision_id
    }

    pub fn package_root(&self) -> &Path {
        &self.package_root
    }

    pub fn expected_content_hash(&self) -> &str {
        &self.expected_content_hash
    }
}

fn validate_bounded_label(
    value: &str,
    maximum_bytes: usize,
    label: &str,
) -> Result<(), SkillResourceError> {
    if value.is_empty()
        || value.len() > maximum_bytes
        || value.chars().any(|character| character.is_control())
    {
        return Err(SkillResourceError::InvalidBinding(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillMediaMetadata {
    pub mime_type: String,
    pub image_dimensions: Option<SkillImageDimensions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillResourceMetadata {
    pub snapshot_generation: u64,
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub revision_content_hash: String,
    pub kind: SkillResourceKind,
    pub path: String,
    pub byte_len: u64,
    pub sha256: String,
    pub media: Option<SkillMediaMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillResourceContent {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillResource {
    metadata: SkillResourceMetadata,
    content: SkillResourceContent,
}

impl SkillResource {
    pub fn metadata(&self) -> &SkillResourceMetadata {
        &self.metadata
    }

    pub fn content(&self) -> &SkillResourceContent {
        &self.content
    }

    pub fn into_content(self) -> SkillResourceContent {
        self.content
    }
}

#[derive(Clone, Debug)]
pub struct SkillResourceReader {
    revision: SkillResourceRevision,
    limits: SkillResourceLimits,
}

impl SkillResourceReader {
    pub fn new(
        revision: SkillResourceRevision,
        limits: SkillResourceLimits,
    ) -> Result<Self, SkillResourceError> {
        Ok(Self {
            revision,
            limits: limits.validate()?,
        })
    }

    pub fn revision(&self) -> &SkillResourceRevision {
        &self.revision
    }

    pub async fn read(
        &self,
        kind: SkillResourceKind,
        path: &SkillResourcePath,
    ) -> Result<SkillResource, SkillResourceError> {
        path.validate_kind(kind)?;
        let path_bytes = u64::try_from(path.canonical.len())
            .map_err(|_| SkillResourceError::InvalidPath(path.canonical.clone()))?;
        let maximum_path_bytes = self
            .limits
            .max_path_bytes
            .min(self.revision.package_limits.max_relative_path_bytes);
        if path_bytes > maximum_path_bytes {
            return Err(SkillResourceError::PathLimitExceeded {
                path: path.canonical.clone(),
                maximum: maximum_path_bytes,
            });
        }
        if !self.revision.known_files.contains(path.canonical()) {
            return Err(SkillResourceError::ResourceNotInRevision {
                revision_id: self.revision.revision_id.clone(),
                path: path.canonical.clone(),
            });
        }

        self.verify_revision().await?;
        let (file, opened_bytes, _) =
            open_regular_file_nofollow(&self.revision.package_root, path.as_path())
                .await
                .map_err(|source| SkillResourceError::Access {
                    path: path.canonical.clone(),
                    source,
                })?;
        let maximum_bytes = self
            .limits
            .byte_limit(kind)
            .min(self.revision.package_limits.max_file_bytes);
        if opened_bytes > maximum_bytes {
            return Err(SkillResourceError::ByteLimitExceeded {
                kind,
                path: path.canonical.clone(),
                maximum: maximum_bytes,
            });
        }
        let capacity =
            usize::try_from(opened_bytes).map_err(|_| SkillResourceError::ByteLimitExceeded {
                kind,
                path: path.canonical.clone(),
                maximum: maximum_bytes,
            })?;
        let mut bytes = Vec::with_capacity(capacity);
        file.take(maximum_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|source| SkillResourceError::Access {
                path: path.canonical.clone(),
                source: source.into(),
            })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
            return Err(SkillResourceError::ByteLimitExceeded {
                kind,
                path: path.canonical.clone(),
                maximum: maximum_bytes,
            });
        }
        if u64::try_from(bytes.len()).ok() != Some(opened_bytes) {
            return Err(SkillResourceError::ChangedDuringRead(
                path.canonical.clone(),
            ));
        }
        self.verify_revision().await?;

        let media = if kind == SkillResourceKind::Asset {
            inspect_media(&bytes, self.limits, path.canonical())?
        } else {
            None
        };
        let content = match self.limits.char_limit(kind) {
            Some(maximum_chars) => {
                let text = std::str::from_utf8(&bytes).map_err(|source| {
                    SkillResourceError::InvalidUtf8 {
                        kind,
                        path: path.canonical.clone(),
                        source,
                    }
                })?;
                if text.contains('\0') {
                    return Err(SkillResourceError::TextContainsNul(path.canonical.clone()));
                }
                if text.chars().count() > maximum_chars {
                    return Err(SkillResourceError::CharacterLimitExceeded {
                        kind,
                        path: path.canonical.clone(),
                        maximum: maximum_chars,
                    });
                }
                SkillResourceContent::Text(text.to_string())
            }
            None => SkillResourceContent::Binary(bytes.clone()),
        };
        Ok(SkillResource {
            metadata: SkillResourceMetadata {
                snapshot_generation: self.revision.snapshot_generation,
                package_id: self.revision.package_id.clone(),
                revision_id: self.revision.revision_id.clone(),
                revision_content_hash: self.revision.expected_content_hash.clone(),
                kind,
                path: path.canonical.clone(),
                byte_len: opened_bytes,
                sha256: hex::encode(Sha256::digest(&bytes)),
                media,
            },
            content,
        })
    }

    async fn verify_revision(&self) -> Result<(), SkillResourceError> {
        if let (Some(lease), Some(revision_id)) =
            (&self.revision.lease, &self.revision.lease_revision_id)
            && let Some(execution_lease) = lease.execution_lease()
        {
            execution_lease
                .authorize_revision(revision_id)
                .await
                .map_err(SkillResourceError::RevisionAuthorization)?;
        }
        let observed = secure_package_hash(
            &self.revision.package_root,
            self.revision.package_limits.package_limits(),
        )
        .await
        .map_err(|source| SkillResourceError::RevisionInspection {
            revision_id: self.revision.revision_id.clone(),
            source,
        })?;
        if observed != self.revision.expected_content_hash {
            return Err(SkillResourceError::RevisionContentMismatch {
                revision_id: self.revision.revision_id.clone(),
                expected: self.revision.expected_content_hash.clone(),
                observed,
            });
        }
        Ok(())
    }
}

fn inspect_media(
    bytes: &[u8],
    limits: SkillResourceLimits,
    path: &str,
) -> Result<Option<SkillMediaMetadata>, SkillResourceError> {
    let (mime_type, dimensions) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return Err(SkillResourceError::InvalidMediaMetadata(path.into()));
        }
        (
            "image/png",
            Some(SkillImageDimensions {
                width: u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
                height: u32::from_be_bytes(bytes[20..24].try_into().unwrap()),
            }),
        )
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if bytes.len() < 10 {
            return Err(SkillResourceError::InvalidMediaMetadata(path.into()));
        }
        (
            "image/gif",
            Some(SkillImageDimensions {
                width: u16::from_le_bytes(bytes[6..8].try_into().unwrap()).into(),
                height: u16::from_le_bytes(bytes[8..10].try_into().unwrap()).into(),
            }),
        )
    } else if bytes.starts_with(b"\xff\xd8") {
        (
            "image/jpeg",
            Some(jpeg_dimensions(bytes, limits.max_media_header_bytes, path)?),
        )
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        ("audio/wav", None)
    } else if bytes.starts_with(b"fLaC") {
        ("audio/flac", None)
    } else if bytes.starts_with(b"OggS") {
        ("application/ogg", None)
    } else if bytes.starts_with(b"ID3") {
        ("audio/mpeg", None)
    } else if bytes.starts_with(b"%PDF-") {
        ("application/pdf", None)
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        ("video/mp4", None)
    } else {
        return Ok(None);
    };
    if let Some(dimensions) = &dimensions {
        validate_dimensions(dimensions, limits, path)?;
    }
    Ok(Some(SkillMediaMetadata {
        mime_type: mime_type.into(),
        image_dimensions: dimensions,
    }))
}

fn jpeg_dimensions(
    bytes: &[u8],
    maximum_header_bytes: usize,
    path: &str,
) -> Result<SkillImageDimensions, SkillResourceError> {
    let scan_end = bytes.len().min(maximum_header_bytes);
    let mut offset = 2;
    while offset + 3 < scan_end {
        if bytes[offset] != 0xff {
            offset += 1;
            continue;
        }
        while offset < scan_end && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= scan_end {
            break;
        }
        let marker = bytes[offset];
        offset += 1;
        if matches!(marker, 0x01 | 0xd8 | 0xd9) {
            continue;
        }
        if offset + 2 > scan_end {
            break;
        }
        let segment_len = usize::from(u16::from_be_bytes(
            bytes[offset..offset + 2].try_into().unwrap(),
        ));
        if segment_len < 2 || offset + segment_len > scan_end {
            break;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            if segment_len < 7 {
                break;
            }
            return Ok(SkillImageDimensions {
                height: u16::from_be_bytes(bytes[offset + 3..offset + 5].try_into().unwrap())
                    .into(),
                width: u16::from_be_bytes(bytes[offset + 5..offset + 7].try_into().unwrap()).into(),
            });
        }
        offset += segment_len;
    }
    if bytes.len() > maximum_header_bytes {
        Err(SkillResourceError::MediaHeaderLimitExceeded {
            path: path.into(),
            maximum: maximum_header_bytes,
        })
    } else {
        Err(SkillResourceError::InvalidMediaMetadata(path.into()))
    }
}

fn validate_dimensions(
    dimensions: &SkillImageDimensions,
    limits: SkillResourceLimits,
    path: &str,
) -> Result<(), SkillResourceError> {
    let pixels = u64::from(dimensions.width) * u64::from(dimensions.height);
    if dimensions.width == 0
        || dimensions.height == 0
        || dimensions.width > limits.max_image_dimension
        || dimensions.height > limits.max_image_dimension
        || pixels > limits.max_image_pixels
    {
        return Err(SkillResourceError::ImageDimensionsExceeded {
            path: path.into(),
            width: dimensions.width,
            height: dimensions.height,
        });
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SkillResourceError {
    #[error("invalid skill resource binding: {0}")]
    InvalidBinding(String),
    #[error("invalid portable skill resource path: {0}")]
    InvalidPath(String),
    #[error("skill resource path {path} does not belong to {kind:?}")]
    KindPathMismatch {
        kind: SkillResourceKind,
        path: String,
    },
    #[error("skill package {0:?} is not in the snapshot")]
    PackageNotInSnapshot(SkillPackageId),
    #[error("skill package {0:?} has no verified snapshot content")]
    UnverifiedPackage(SkillPackageId),
    #[error("invalid skill resource limits")]
    InvalidLimits,
    #[error("skill resource path {path} exceeds {maximum} bytes")]
    PathLimitExceeded { path: String, maximum: u64 },
    #[error("resource {path} was not present in revision {revision_id}")]
    ResourceNotInRevision { revision_id: String, path: String },
    #[error("skill revision authorization failed")]
    RevisionAuthorization(#[source] anyhow::Error),
    #[error("failed to inspect skill revision {revision_id}")]
    RevisionInspection {
        revision_id: String,
        #[source]
        source: anyhow::Error,
    },
    #[error(
        "skill revision {revision_id} content mismatch: expected {expected}, observed {observed}"
    )]
    RevisionContentMismatch {
        revision_id: String,
        expected: String,
        observed: String,
    },
    #[error("failed to access skill resource {path}")]
    Access {
        path: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("{kind:?} resource {path} exceeds {maximum} bytes")]
    ByteLimitExceeded {
        kind: SkillResourceKind,
        path: String,
        maximum: u64,
    },
    #[error("skill resource changed while being read: {0}")]
    ChangedDuringRead(String),
    #[error("{kind:?} resource {path} is not valid UTF-8")]
    InvalidUtf8 {
        kind: SkillResourceKind,
        path: String,
        #[source]
        source: std::str::Utf8Error,
    },
    #[error("text skill resource contains a nul byte: {0}")]
    TextContainsNul(String),
    #[error("{kind:?} resource {path} exceeds {maximum} characters")]
    CharacterLimitExceeded {
        kind: SkillResourceKind,
        path: String,
        maximum: usize,
    },
    #[error("invalid or incomplete media metadata: {0}")]
    InvalidMediaMetadata(String),
    #[error("media header for {path} exceeds the {maximum} byte inspection limit")]
    MediaHeaderLimitExceeded { path: String, maximum: usize },
    #[error("image dimensions for {path} are outside policy: {width}x{height}")]
    ImageDimensionsExceeded {
        path: String,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxSkillHelperRuntime {
    Python,
    JavaScript,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SandboxSkillHelperLimits {
    pub max_args: usize,
    pub max_argument_bytes: usize,
    pub max_stdin_bytes: usize,
    pub max_output_bytes: usize,
    pub max_timeout_ms: u64,
}

impl Default for SandboxSkillHelperLimits {
    fn default() -> Self {
        Self {
            max_args: 64,
            max_argument_bytes: 32 * 1024,
            max_stdin_bytes: 1024 * 1024,
            max_output_bytes: 2 * 1024 * 1024,
            max_timeout_ms: 60_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SandboxSkillHelperRequest {
    runtime: SandboxSkillHelperRuntime,
    script: SkillResource,
    args: Vec<String>,
    stdin: Vec<u8>,
    timeout_ms: u64,
    max_output_bytes: usize,
}

impl SandboxSkillHelperRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runtime: SandboxSkillHelperRuntime,
        script: SkillResource,
        args: Vec<String>,
        stdin: Vec<u8>,
        timeout_ms: u64,
        max_output_bytes: usize,
        limits: SandboxSkillHelperLimits,
    ) -> Result<Self, SandboxSkillHelperError> {
        if script.metadata.kind != SkillResourceKind::Script
            || !matches!(&script.content, SkillResourceContent::Text(_))
        {
            return Err(SandboxSkillHelperError::InvalidRequest(
                "helper input must be a verified text script resource".into(),
            ));
        }
        let extension = Path::new(&script.metadata.path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let runtime_matches = match runtime {
            SandboxSkillHelperRuntime::Python => extension == "py",
            SandboxSkillHelperRuntime::JavaScript => {
                matches!(extension.as_str(), "js" | "mjs" | "cjs")
            }
        };
        if !runtime_matches {
            return Err(SandboxSkillHelperError::InvalidRequest(
                "helper runtime does not match the verified script extension".into(),
            ));
        }
        if args.len() > limits.max_args
            || args.iter().any(|arg| arg.contains('\0'))
            || args
                .iter()
                .try_fold(0_usize, |total, arg| total.checked_add(arg.len()))
                .is_none_or(|total| total > limits.max_argument_bytes)
            || stdin.len() > limits.max_stdin_bytes
            || timeout_ms == 0
            || timeout_ms > limits.max_timeout_ms
            || max_output_bytes == 0
            || max_output_bytes > limits.max_output_bytes
        {
            return Err(SandboxSkillHelperError::InvalidRequest(
                "helper arguments, input, timeout, or output cap exceed policy".into(),
            ));
        }
        Ok(Self {
            runtime,
            script,
            args,
            stdin,
            timeout_ms,
            max_output_bytes,
        })
    }

    pub fn runtime(&self) -> SandboxSkillHelperRuntime {
        self.runtime
    }

    pub fn script(&self) -> &SkillResource {
        &self.script
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn stdin(&self) -> &[u8] {
        &self.stdin
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxSkillHelperOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SandboxSkillHelperOutput {
    pub fn bounded(
        exit_code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        maximum: usize,
    ) -> Result<Self, SandboxSkillHelperError> {
        let total = stdout.len().checked_add(stderr.len()).ok_or_else(|| {
            SandboxSkillHelperError::InvalidOutput("output length overflow".into())
        })?;
        if total > maximum {
            return Err(SandboxSkillHelperError::InvalidOutput(format!(
                "helper output exceeds {maximum} bytes"
            )));
        }
        Ok(Self {
            exit_code,
            stdout,
            stderr,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SandboxSkillHelperError {
    #[error("sandbox skill helper execution is disabled")]
    Disabled,
    #[error("invalid sandbox skill helper request: {0}")]
    InvalidRequest(String),
    #[error("invalid sandbox skill helper output: {0}")]
    InvalidOutput(String),
    #[error("sandbox skill helper execution failed: {0}")]
    ExecutionFailed(String),
}

#[async_trait]
pub trait SandboxSkillHelperExecutor: Send + Sync {
    async fn execute(
        &self,
        request: &SandboxSkillHelperRequest,
    ) -> Result<SandboxSkillHelperOutput, SandboxSkillHelperError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DisabledSandboxSkillHelperExecutor;

#[async_trait]
impl SandboxSkillHelperExecutor for DisabledSandboxSkillHelperExecutor {
    async fn execute(
        &self,
        _request: &SandboxSkillHelperRequest,
    ) -> Result<SandboxSkillHelperOutput, SandboxSkillHelperError> {
        Err(SandboxSkillHelperError::Disabled)
    }
}

#[cfg(test)]
#[path = "skill_resource_tests.rs"]
mod tests;
