use anyhow::Context;
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub const SKILL_PACKAGE_SCHEMA_VERSION: u32 = 1;
const MAX_SKILL_PACKAGE_ID_LENGTH: usize = 128;
const LEGACY_PACKAGE_ID_PREFIX: &str = "legacy.local.";
const LEGACY_PACKAGE_HASH_LENGTH: usize = 12;

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPackageId(String);

impl SkillPackageId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let valid = value.len() <= MAX_SKILL_PACKAGE_ID_LENGTH
            && value.split('.').count() >= 3
            && value.split('.').all(is_valid_package_id_segment);
        if !valid {
            anyhow::bail!("invalid skill package id: {value}");
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SkillPackageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillPackageKind {
    InstructionOnly,
    HostToolsOnly,
    NativeRuntime,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPackageTargets {
    pub include_instructions: bool,
    pub include_runtime: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillCompatibility {
    pub minimum_runtime_version: Option<Version>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillPackageRequirements {
    #[serde(default)]
    pub packages: Vec<SkillPackageId>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub runtime_tools: Vec<String>,
    #[serde(default)]
    pub connectors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPackageDescriptor {
    pub schema_version: u32,
    pub id: SkillPackageId,
    pub version: Version,
    pub display_name: String,
    pub kind: SkillPackageKind,
    pub package: SkillPackageTargets,
    #[serde(default)]
    pub compatibility: SkillCompatibility,
    #[serde(default)]
    pub requires: SkillPackageRequirements,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillPackageDescriptorWire {
    schema_version: u32,
    id: SkillPackageId,
    version: Version,
    display_name: String,
    kind: SkillPackageKind,
    package: SkillPackageTargets,
    #[serde(default)]
    compatibility: SkillCompatibility,
    #[serde(default)]
    requires: SkillPackageRequirements,
}

impl TryFrom<SkillPackageDescriptorWire> for SkillPackageDescriptor {
    type Error = anyhow::Error;

    fn try_from(wire: SkillPackageDescriptorWire) -> Result<Self, Self::Error> {
        if wire.schema_version != SKILL_PACKAGE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported skill package schema version: {}",
                wire.schema_version
            );
        }
        Ok(Self {
            schema_version: wire.schema_version,
            id: wire.id,
            version: wire.version,
            display_name: wire.display_name,
            kind: wire.kind,
            package: wire.package,
            compatibility: wire.compatibility,
            requires: wire.requires,
        })
    }
}

impl<'de> Deserialize<'de> for SkillPackageDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        SkillPackageDescriptorWire::deserialize(deserializer)?
            .try_into()
            .map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorSource {
    Explicit,
    LegacySynthesized,
}

#[derive(Clone, Debug)]
pub struct LoadedPackageDescriptor {
    pub root: PathBuf,
    pub descriptor: SkillPackageDescriptor,
    pub source: DescriptorSource,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPackageMetadata {
    #[serde(default)]
    package: Option<LegacyPackageTargets>,
    #[serde(default)]
    requires: LegacyPackageRequirements,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPackageTargets {
    #[serde(default)]
    include_instructions: Option<bool>,
    #[serde(default)]
    include_runtime: Option<bool>,
}

impl LegacyPackageTargets {
    fn resolve(self, inferred: &SkillPackageTargets) -> SkillPackageTargets {
        SkillPackageTargets {
            include_instructions: self
                .include_instructions
                .unwrap_or(inferred.include_instructions),
            include_runtime: self.include_runtime.unwrap_or(inferred.include_runtime),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPackageRequirements {
    #[serde(default)]
    packages: Vec<SkillPackageId>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    runtime_tools: Vec<String>,
    #[serde(default)]
    connectors: Vec<String>,
}

impl From<LegacyPackageRequirements> for SkillPackageRequirements {
    fn from(legacy: LegacyPackageRequirements) -> Self {
        Self {
            packages: legacy.packages,
            capabilities: legacy.capabilities,
            runtime_tools: legacy.runtime_tools,
            connectors: legacy.connectors,
        }
    }
}

impl SkillPackageDescriptor {
    pub async fn load(package_root: &Path) -> anyhow::Result<LoadedPackageDescriptor> {
        let root_metadata = tokio::fs::metadata(package_root).await.with_context(|| {
            format!(
                "failed to inspect skill package root {}",
                package_root.display()
            )
        })?;
        if !root_metadata.is_dir() {
            anyhow::bail!(
                "skill package root must be a directory: {}",
                package_root.display()
            );
        }

        let descriptor_path = package_root.join("general-agent.json");
        if let Some(bytes) = read_optional_file(&descriptor_path).await? {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            if value.get("schemaVersion").is_some() {
                let descriptor: SkillPackageDescriptor = serde_json::from_value(value)?;
                return Ok(LoadedPackageDescriptor {
                    root: package_root.to_path_buf(),
                    descriptor,
                    source: DescriptorSource::Explicit,
                    warnings: Vec::new(),
                });
            }
            let metadata: LegacyPackageMetadata = serde_json::from_value(value)?;
            return load_legacy_descriptor(package_root, Some(metadata)).await;
        }

        load_legacy_descriptor(package_root, None).await
    }
}

async fn load_legacy_descriptor(
    package_root: &Path,
    metadata: Option<LegacyPackageMetadata>,
) -> anyhow::Result<LoadedPackageDescriptor> {
    let folder = package_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("legacy package folder must be UTF-8"))?;
    let id = legacy_package_id(folder)?;
    let runtime_manifest = read_optional_file(&package_root.join("skill.json")).await?;
    let instructions_file = read_optional_file(&package_root.join("SKILL.md")).await?;
    if runtime_manifest.is_none() && instructions_file.is_none() {
        anyhow::bail!(
            "legacy skill package must contain SKILL.md or skill.json: {}",
            package_root.display()
        );
    }
    let runtime = runtime_manifest.is_some();
    let instructions = instructions_file.is_some();
    let inferred_package = SkillPackageTargets {
        include_instructions: instructions,
        include_runtime: runtime,
    };
    let (package, requires) = metadata
        .map(|metadata| {
            (
                metadata
                    .package
                    .map(|package| package.resolve(&inferred_package))
                    .unwrap_or_else(|| inferred_package.clone()),
                metadata.requires.into(),
            )
        })
        .unwrap_or((inferred_package, SkillPackageRequirements::default()));
    let descriptor = SkillPackageDescriptor {
        schema_version: SKILL_PACKAGE_SCHEMA_VERSION,
        id,
        version: legacy_version(runtime_manifest.as_deref())?,
        display_name: folder.to_string(),
        kind: if package.include_runtime {
            SkillPackageKind::NativeRuntime
        } else if package.include_instructions {
            SkillPackageKind::InstructionOnly
        } else {
            SkillPackageKind::HostToolsOnly
        },
        package,
        compatibility: SkillCompatibility::default(),
        requires,
    };
    Ok(LoadedPackageDescriptor {
        root: package_root.to_path_buf(),
        descriptor,
        source: DescriptorSource::LegacySynthesized,
        warnings: vec!["legacy package descriptor synthesized; add general-agent.json".into()],
    })
}

fn is_valid_package_id_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn legacy_package_id(folder: &str) -> anyhow::Result<SkillPackageId> {
    let maximum_suffix_length = MAX_SKILL_PACKAGE_ID_LENGTH - LEGACY_PACKAGE_ID_PREFIX.len();
    if folder.len() <= maximum_suffix_length && is_valid_package_id_segment(folder) {
        return SkillPackageId::parse(&format!("{LEGACY_PACKAGE_ID_PREFIX}{folder}"));
    }

    let mut slug = String::with_capacity(folder.len());
    for ch in folder.chars() {
        let normalized = if ch.is_ascii_uppercase() {
            ch.to_ascii_lowercase()
        } else if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
            ch
        } else {
            '-'
        };
        if normalized != '-' || !slug.ends_with('-') {
            slug.push(normalized);
        }
    }

    let digest = hex::encode(Sha256::digest(folder.as_bytes()));
    let short_hash = &digest[..LEGACY_PACKAGE_HASH_LENGTH];
    let maximum_slug_length = maximum_suffix_length - short_hash.len() - 1;
    let trimmed = slug.trim_matches('-');
    let mut bounded_slug = if trimmed.is_empty() {
        "package".to_string()
    } else {
        trimmed[..trimmed.len().min(maximum_slug_length)].to_string()
    };
    while bounded_slug.ends_with('-') {
        bounded_slug.pop();
    }
    if bounded_slug.is_empty() {
        bounded_slug.push_str("package");
    }

    SkillPackageId::parse(&format!(
        "{LEGACY_PACKAGE_ID_PREFIX}{bounded_slug}-{short_hash}"
    ))
}

async fn read_optional_file(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let entry_metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect package file {}", path.display()));
        }
    };

    if entry_metadata.file_type().is_symlink() {
        let target_metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("failed to inspect package file target {}", path.display()))?;
        if !target_metadata.is_file() {
            anyhow::bail!("package path must be a file: {}", path.display());
        }
    } else if !entry_metadata.is_file() {
        anyhow::bail!("package path must be a file: {}", path.display());
    }

    tokio::fs::read(path)
        .await
        .map(Some)
        .with_context(|| format!("failed to read package file {}", path.display()))
}

fn legacy_version(runtime_manifest: Option<&[u8]>) -> anyhow::Result<Version> {
    let Some(bytes) = runtime_manifest else {
        return Ok(Version::new(0, 0, 0));
    };
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(Version::parse)
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("legacy runtime package version is required"))
}
