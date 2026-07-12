use crate::skill_package::{
    SkillPackageId, SkillPackageKind, SkillPackageRequirements, SkillPackageTargets,
};
use crate::skill_source::DiscoveredSkillPackage;
use crate::{platform::PlatformId, skill_source::SkillSource};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[path = "skill_bundle_builder.rs"]
mod builder;
#[cfg(test)]
#[path = "skill_bundle_builder_gates.rs"]
mod builder_gates;
#[path = "skill_bundle_source.rs"]
mod source;

pub use builder::build_skill_bundle;
#[cfg(test)]
pub(crate) use builder::build_skill_bundle_with_faults;
#[cfg(test)]
pub(crate) use builder_gates::{
    gate_bundle_after_final_validation, gate_bundle_after_inspection, gate_bundle_before_publish,
};
pub use source::BundleSkillSource;
#[cfg(all(test, windows))]
pub(crate) use source::gate_bundle_current_after_open;
#[cfg(test)]
pub(crate) use source::gate_bundle_discovery_after_layout;
#[cfg(all(test, unix))]
pub(crate) use source::gate_bundle_metadata_after_inspection;
pub(crate) use source::verify_bundle_generation_binding;

pub const SKILL_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub(crate) const SKILL_BUNDLE_CURRENT_SCHEMA_VERSION: u32 = 2;
pub const SKILL_BUNDLE_MANIFEST_FILE: &str = "skill-bundle.json";
pub const SKILL_BUNDLE_LOCK_FILE: &str = "skill-bundle.lock";
pub const SKILL_BUNDLE_CURRENT_FILE: &str = "current";
pub const SKILL_BUNDLE_GENERATIONS_DIR: &str = "generations";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillBundleCurrent {
    pub(crate) schema_version: u32,
    pub(crate) active: SkillBundleGeneration,
    pub(crate) previous: Option<SkillBundleGeneration>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SkillBundleGeneration {
    pub(crate) generation: String,
    pub(crate) manifest_sha256: String,
    pub(crate) lock_sha256: String,
}

#[derive(Clone, Debug)]
pub struct BuildSkillBundleRequest {
    pub source_roots: Vec<PathBuf>,
    pub output_root: PathBuf,
    pub platform: PlatformId,
    pub runtime_version: Version,
    pub generated_at: String,
}

#[derive(Clone, Debug)]
pub struct BuildSkillBundleResult {
    pub root: PathBuf,
    pub package_count: usize,
    pub manifest_bytes: Vec<u8>,
    pub lock_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillBundleManifest {
    pub schema_version: u32,
    pub generated_at: String,
    pub packages: Vec<SkillBundlePackage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillBundlePackage {
    pub id: SkillPackageId,
    pub version: Version,
    pub display_name: String,
    pub kind: SkillPackageKind,
    pub path: PathBuf,
    pub content_hash: String,
    pub include_instructions: bool,
    pub include_runtime: bool,
    pub minimum_runtime_version: Option<Version>,
    pub platforms: Vec<String>,
    pub capabilities: Vec<String>,
    pub runtime_tools: Vec<String>,
    pub connectors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillBundleLock {
    pub schema_version: u32,
    pub packages: Vec<SkillBundleLockPackage>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillBundleLockPackage {
    pub id: SkillPackageId,
    pub version: Version,
    pub content_hash: String,
    pub dependencies: Vec<SkillPackageId>,
}

impl SkillBundlePackage {
    pub(crate) fn targets(&self) -> SkillPackageTargets {
        SkillPackageTargets {
            include_instructions: self.include_instructions,
            include_runtime: self.include_runtime,
        }
    }

    pub(crate) fn requirements(
        &self,
        dependencies: Vec<SkillPackageId>,
    ) -> SkillPackageRequirements {
        SkillPackageRequirements {
            packages: dependencies,
            capabilities: self.capabilities.clone(),
            runtime_tools: self.runtime_tools.clone(),
            connectors: self.connectors.clone(),
        }
    }
}

impl BundleSkillSource {
    pub async fn packages(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        self.discover().await
    }
}
