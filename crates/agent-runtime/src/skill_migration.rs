use crate::skill_package::{SkillPackageDescriptor, SkillPackageId, SkillPackageKind};
use crate::skill_source::{DirectorySkillSource, SkillLayer};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LegacySkillMigrationDiagnostic {
    pub package_path: PathBuf,
    pub synthesized_package_id: SkillPackageId,
    pub inferred_kind: SkillPackageKind,
    pub recommended_descriptor: SkillPackageDescriptor,
}

pub async fn scan_legacy_packages(
    root: &Path,
) -> anyhow::Result<Vec<LegacySkillMigrationDiagnostic>> {
    let report = DirectorySkillSource::new(SkillLayer::Builtin, root)
        .discover_release()
        .await?;
    Ok(diagnostics_from_packages(report.packages))
}

pub fn diagnostics_from_packages(
    packages: impl IntoIterator<Item = crate::skill_source::DiscoveredSkillPackage>,
) -> Vec<LegacySkillMigrationDiagnostic> {
    let mut diagnostics = packages
        .into_iter()
        .filter(|package| {
            package
                .warnings
                .iter()
                .any(|warning| warning.contains("legacy package descriptor synthesized"))
        })
        .map(|package| LegacySkillMigrationDiagnostic {
            package_path: package.root,
            synthesized_package_id: package.descriptor.id.clone(),
            inferred_kind: package.descriptor.kind,
            recommended_descriptor: package.descriptor,
        })
        .collect::<Vec<_>>();
    diagnostics.sort_by(|left, right| left.package_path.cmp(&right.package_path));
    diagnostics
}
