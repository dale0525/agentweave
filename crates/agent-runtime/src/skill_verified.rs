use crate::skill::{
    InstalledSkill, InstalledSkillVerification, SkillManifest, SkillRegistry, canonical_skill_root,
    validate_manifest,
};
use crate::skill_store_fs::PackageLimits;
use crate::skill_store_secure_fs::secure_package_hash;
use anyhow::Context;
use std::path::PathBuf;

impl SkillRegistry {
    pub(crate) async fn load_verified_skill(
        root: PathBuf,
        manifest_bytes: &[u8],
        expected_content_hash: String,
        limits: PackageLimits,
    ) -> anyhow::Result<InstalledSkill> {
        let root = canonical_skill_root(&root).await?;
        let manifest: SkillManifest =
            serde_json::from_slice(manifest_bytes).with_context(|| {
                format!("failed to parse verified skill manifest {}", root.display())
            })?;
        validate_manifest(&root, &manifest).await?;
        Ok(InstalledSkill {
            root,
            manifest,
            verification: Some(InstalledSkillVerification {
                expected_content_hash,
                limits,
            }),
        })
    }
}

pub(crate) async fn verify_before_execution(skill: &InstalledSkill) -> anyhow::Result<()> {
    let Some(verification) = &skill.verification else {
        return Ok(());
    };
    let actual = secure_package_hash(&skill.root, verification.limits).await?;
    if actual != verification.expected_content_hash {
        anyhow::bail!(
            "managed skill content changed since managed snapshot: {}",
            skill.root.display()
        );
    }
    Ok(())
}
