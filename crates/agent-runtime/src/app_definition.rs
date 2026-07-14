use crate::app_manifest::{AgentAppManifest, LoadedAgentAppManifest};
use crate::platform::PlatformId;
use crate::prompt_composer::AppPromptConfig;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAppRuntimeInventory {
    pub runtime_version: Version,
    pub platform: PlatformId,
    pub packages: BTreeMap<String, Version>,
    pub capabilities: BTreeSet<String>,
    pub runtime_tools: BTreeSet<String>,
    pub connectors: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct ResolvedAgentApp {
    pub loaded: LoadedAgentAppManifest,
    pub prompt: AppPromptConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentAppDiagnostics {
    pub app_id: String,
    pub version: String,
    pub display_name: String,
    pub manifest_sha256: String,
}

impl ResolvedAgentApp {
    pub async fn load(
        root: &Path,
        inventory: &AgentAppRuntimeInventory,
        max_prompt_resource_bytes: usize,
    ) -> anyhow::Result<Self> {
        let loaded = AgentAppManifest::load(root).await?;
        validate_app_compatibility(&loaded.manifest, inventory)?;
        let prompt =
            AppPromptConfig::from_loaded_manifest(&loaded, max_prompt_resource_bytes).await?;
        Ok(Self { loaded, prompt })
    }

    pub fn diagnostics(&self) -> AgentAppDiagnostics {
        AgentAppDiagnostics {
            app_id: self.prompt.identity.app_id.clone(),
            version: self.prompt.identity.version.clone(),
            display_name: self.prompt.identity.display_name.clone(),
            manifest_sha256: self.loaded.manifest_sha256().to_string(),
        }
    }
}

pub fn validate_app_compatibility(
    manifest: &AgentAppManifest,
    inventory: &AgentAppRuntimeInventory,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        manifest.supports_platform(inventory.platform),
        "Agent App {} does not support platform {}",
        manifest.app_id.as_str(),
        platform_name(inventory.platform)
    );
    if let Some(requirement) = &manifest.compatibility.runtime {
        anyhow::ensure!(
            requirement.matches(&inventory.runtime_version),
            "Agent App requires runtime {requirement}, found {}",
            inventory.runtime_version
        );
    }
    for requirement in &manifest.requires.packages {
        let version = inventory
            .packages
            .get(requirement.id.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "required App package is unavailable: {}",
                    requirement.id.as_str()
                )
            })?;
        anyhow::ensure!(
            requirement.version.matches(version),
            "required App package {} expects {}, found {version}",
            requirement.id.as_str(),
            requirement.version
        );
    }
    validate_required_set(
        "capability",
        manifest
            .requires
            .capabilities
            .iter()
            .map(|item| item.as_str()),
        &inventory.capabilities,
    )?;
    validate_required_set(
        "runtime tool",
        manifest
            .requires
            .runtime_tools
            .iter()
            .map(|item| item.as_str()),
        &inventory.runtime_tools,
    )?;
    validate_required_set(
        "connector",
        manifest
            .requires
            .connectors
            .iter()
            .map(|item| item.as_str()),
        &inventory.connectors,
    )?;
    Ok(())
}

fn validate_required_set<'a>(
    label: &str,
    required: impl Iterator<Item = &'a str>,
    available: &BTreeSet<String>,
) -> anyhow::Result<()> {
    for item in required {
        anyhow::ensure!(
            available.contains(item),
            "required App {label} is unavailable: {item}"
        );
    }
    Ok(())
}

fn platform_name(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Desktop => "desktop",
        PlatformId::Android => "android",
        PlatformId::Ios => "ios",
        PlatformId::Web => "web",
        PlatformId::Server => "server",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_manifest::AgentAppManifest;

    fn manifest() -> AgentAppManifest {
        AgentAppManifest::parse_json(
            br##"{
              "schemaVersion": 1,
              "appId": "com.example.secretary",
              "package": {"id": "com.example.secretary.app", "version": "1.0.0"},
              "compatibility": {"runtime": ">=0.1.0, <1.0.0", "platforms": ["server"]},
              "requires": {
                "packages": [{"id": "agentweave.foundation.memory", "version": "^0.1"}],
                "capabilities": ["memory.read"],
                "runtimeTools": ["memory.search"],
                "connectors": ["mail.fake"]
              },
              "policy": {
                "externalSideEffects": "require_approval",
                "network": "declared_only",
                "backgroundExecution": "disabled",
                "memoryPersistence": "local_only",
                "skillManagement": "disabled"
              },
              "branding": {"displayName": "Secretary"},
              "instructions": {"system": "prompts/system.md"}
            }"##,
        )
        .unwrap()
    }

    fn inventory_fixture() -> AgentAppRuntimeInventory {
        AgentAppRuntimeInventory {
            runtime_version: "0.1.0".parse().unwrap(),
            platform: PlatformId::Server,
            packages: BTreeMap::from([(
                "agentweave.foundation.memory".into(),
                "0.1.2".parse().unwrap(),
            )]),
            capabilities: BTreeSet::from(["memory.read".into()]),
            runtime_tools: BTreeSet::from(["memory.search".into()]),
            connectors: BTreeSet::from(["mail.fake".into()]),
        }
    }

    #[test]
    fn compatible_inventory_is_accepted() {
        validate_app_compatibility(&manifest(), &inventory_fixture()).unwrap();
    }

    #[test]
    fn missing_or_incompatible_requirements_fail_closed() {
        let mut inventory = inventory_fixture();
        inventory.packages.clear();
        assert!(
            validate_app_compatibility(&manifest(), &inventory)
                .unwrap_err()
                .to_string()
                .contains("package")
        );
        let mut inventory = inventory_fixture();
        inventory.capabilities.clear();
        assert!(
            validate_app_compatibility(&manifest(), &inventory)
                .unwrap_err()
                .to_string()
                .contains("capability")
        );
    }

    #[test]
    fn platform_and_future_runtime_mismatches_fail_closed() {
        let mut inventory = inventory_fixture();
        inventory.platform = PlatformId::Android;
        assert!(validate_app_compatibility(&manifest(), &inventory).is_err());
        let mut inventory = inventory_fixture();
        inventory.runtime_version = "2.0.0".parse().unwrap();
        assert!(validate_app_compatibility(&manifest(), &inventory).is_err());
    }
}
