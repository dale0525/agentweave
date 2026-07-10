use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SKILL_PACKAGE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPackageId(String);

impl SkillPackageId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let valid = value.len() <= 128
            && value.split('.').count() >= 3
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
            });
        if !valid {
            anyhow::bail!("invalid skill package id: {value}");
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
#[serde(rename_all = "camelCase")]
pub struct SkillPackageTargets {
    pub include_instructions: bool,
    pub include_runtime: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillCompatibility {
    pub minimum_runtime_version: Option<Version>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

impl SkillPackageDescriptor {
    pub async fn load(package_root: &Path) -> anyhow::Result<LoadedPackageDescriptor> {
        let descriptor_path = package_root.join("general-agent.json");
        if descriptor_path.is_file() {
            let bytes = tokio::fs::read(&descriptor_path).await?;
            let descriptor: SkillPackageDescriptor = serde_json::from_slice(&bytes)?;
            if descriptor.schema_version != SKILL_PACKAGE_SCHEMA_VERSION {
                anyhow::bail!(
                    "unsupported skill package schema version: {}",
                    descriptor.schema_version
                );
            }
            SkillPackageId::parse(descriptor.id.as_str())?;
            return Ok(LoadedPackageDescriptor {
                root: package_root.to_path_buf(),
                descriptor,
                source: DescriptorSource::Explicit,
                warnings: Vec::new(),
            });
        }

        let folder = package_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("legacy package folder must be UTF-8"))?;
        let normalized: String = folder
            .chars()
            .map(|ch| {
                if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
                    ch
                } else {
                    '-'
                }
            })
            .collect();
        let id = SkillPackageId::parse(&format!("legacy.local.{normalized}"))?;
        let runtime = package_root.join("skill.json").is_file();
        let instructions = package_root.join("SKILL.md").is_file();
        let descriptor = SkillPackageDescriptor {
            schema_version: SKILL_PACKAGE_SCHEMA_VERSION,
            id,
            version: legacy_version(package_root).await?,
            display_name: folder.to_string(),
            kind: if runtime {
                SkillPackageKind::NativeRuntime
            } else {
                SkillPackageKind::InstructionOnly
            },
            package: SkillPackageTargets {
                include_instructions: instructions,
                include_runtime: runtime,
            },
            compatibility: SkillCompatibility::default(),
            requires: SkillPackageRequirements::default(),
        };
        Ok(LoadedPackageDescriptor {
            root: package_root.to_path_buf(),
            descriptor,
            source: DescriptorSource::LegacySynthesized,
            warnings: vec!["legacy package descriptor synthesized; add general-agent.json".into()],
        })
    }
}

async fn legacy_version(package_root: &Path) -> anyhow::Result<Version> {
    let path = package_root.join("skill.json");
    if !path.is_file() {
        return Ok(Version::new(0, 0, 0));
    }
    let value: serde_json::Value = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(Version::parse)
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("legacy runtime package version is required"))
}
