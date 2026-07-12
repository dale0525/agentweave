use crate::skill::{
    InstalledSkill, InstalledSkillVerification, SkillManifest, SkillRegistry,
    validate_manifest_semantics,
};
use crate::skill_store_execution::PreparedSkillExecution;
use crate::skill_store_fs::PackageLimits;
use crate::skill_store_secure_fs::secure_package_hash;
use anyhow::Context;
use std::path::PathBuf;

impl SkillRegistry {
    pub(crate) async fn load_verified_skill(
        root: PathBuf,
        manifest_bytes: &[u8],
        package_id: &crate::skill_package::SkillPackageId,
        expected_content_hash: String,
        limits: PackageLimits,
        execution_binding: Option<crate::skill_source::ManagedExecutionBinding>,
    ) -> anyhow::Result<InstalledSkill> {
        let manifest: SkillManifest =
            serde_json::from_slice(manifest_bytes).with_context(|| {
                format!("failed to parse verified skill manifest {}", root.display())
            })?;
        validate_manifest_semantics(&manifest)?;
        Ok(InstalledSkill {
            root,
            manifest,
            verification: Some(InstalledSkillVerification {
                expected_content_hash,
                limits,
                execution_binding,
            }),
            development_package_id: Some(package_id.as_str().to_string()),
        })
    }
}

pub(crate) async fn prepare_before_execution(
    skill: &InstalledSkill,
) -> anyhow::Result<Option<PreparedSkillExecution>> {
    let Some(verification) = &skill.verification else {
        return Ok(None);
    };
    if let Some(binding) = verification.execution_binding.as_ref() {
        return binding
            .store
            .prepare_managed_execution(
                &binding.package_id,
                &binding.revision_id,
                &binding.storage_path,
                &verification.expected_content_hash,
                verification.limits,
                &skill.manifest,
            )
            .await
            .map(Some);
    }
    let current = secure_package_hash(&skill.root, verification.limits).await?;
    anyhow::ensure!(
        current == verification.expected_content_hash,
        "verified builtin skill content hash mismatch"
    );
    crate::skill::validate_manifest(&skill.root, &skill.manifest).await?;
    Ok(None)
}
