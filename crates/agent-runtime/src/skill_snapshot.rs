use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::{SkillCatalog, SkillCatalogEntry};
use crate::skill_resolver::{ResolvedSkillPackage, ResolvedSkillSet};

#[derive(Clone, Debug)]
pub struct SkillSnapshot {
    generation: u64,
    packages: Vec<ResolvedSkillPackage>,
    inactive: Vec<ResolvedSkillPackage>,
    registry: SkillRegistry,
    catalog: SkillCatalog,
}

impl SkillSnapshot {
    pub async fn build(generation: u64, resolved: ResolvedSkillSet) -> anyhow::Result<Self> {
        let registry = build_registry(&resolved.active).await?;
        let catalog = build_catalog(&resolved.active).await?;
        Ok(Self {
            generation,
            packages: resolved.active,
            inactive: resolved.inactive,
            registry,
            catalog,
        })
    }

    pub fn from_registry_and_catalog(
        generation: u64,
        registry: SkillRegistry,
        catalog: SkillCatalog,
    ) -> Self {
        Self {
            generation,
            packages: Vec::new(),
            inactive: Vec::new(),
            registry,
            catalog,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn packages(&self) -> &[ResolvedSkillPackage] {
        &self.packages
    }

    pub fn inactive(&self) -> &[ResolvedSkillPackage] {
        &self.inactive
    }

    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    pub fn catalog(&self) -> &SkillCatalog {
        &self.catalog
    }

    pub(crate) fn with_platform_capabilities(
        mut self,
        platform: PlatformId,
        capabilities: CapabilitySet,
    ) -> Self {
        self.registry = self
            .registry
            .with_platform_capabilities(platform, capabilities);
        self
    }
}

async fn build_registry(packages: &[ResolvedSkillPackage]) -> anyhow::Result<SkillRegistry> {
    let mut skills = Vec::new();
    for resolved in packages {
        if resolved.package.descriptor.package.include_runtime {
            skills.push(SkillRegistry::load_development_skill(&resolved.package.root).await?);
        }
    }
    SkillRegistry::from_installed(skills)
}

async fn build_catalog(packages: &[ResolvedSkillPackage]) -> anyhow::Result<SkillCatalog> {
    let mut entries = Vec::<SkillCatalogEntry>::new();
    for resolved in packages {
        if resolved.package.descriptor.package.include_instructions {
            entries.push(SkillCatalog::read_package_entry(&resolved.package.root).await?);
        }
    }
    SkillCatalog::from_entries(entries)
}
