use crate::skill::{
    InstalledSkill, InstalledSkillVerification, SkillManifest, SkillRegistry,
    validate_manifest_semantics,
};
use crate::skill_store_execution::PreparedSkillExecution;
use crate::skill_store_fs::PackageLimits;
use crate::skill_store_secure_fs::secure_package_hash;
use anyhow::Context;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub(crate) enum VerifiedExecutionBinding {
    Managed(Box<crate::skill_source::ManagedExecutionBinding>),
    Bundle(Box<crate::skill_source::BundleExecutionBinding>),
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct BundleExecutionGate {
    entered: std::sync::Arc<tokio::sync::Barrier>,
    release: std::sync::Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
impl BundleExecutionGate {
    pub(crate) async fn wait_entered(&self) {
        self.entered.wait().await;
    }

    pub(crate) async fn release(&self) {
        self.release.wait().await;
    }
}

#[cfg(test)]
pub(crate) fn gate_bundle_execution_after_snapshot() -> BundleExecutionGate {
    let gate = BundleExecutionGate {
        entered: std::sync::Arc::new(tokio::sync::Barrier::new(2)),
        release: std::sync::Arc::new(tokio::sync::Barrier::new(2)),
    };
    *bundle_execution_gate().lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn bundle_execution_gate() -> &'static std::sync::Mutex<Option<BundleExecutionGate>> {
    static GATE: std::sync::OnceLock<std::sync::Mutex<Option<BundleExecutionGate>>> =
        std::sync::OnceLock::new();
    GATE.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
async fn checkpoint_bundle_execution_after_snapshot() {
    let gate = bundle_execution_gate().lock().unwrap().take();
    if let Some(gate) = gate {
        gate.entered.wait().await;
        gate.release.wait().await;
    }
}

#[cfg(not(test))]
async fn checkpoint_bundle_execution_after_snapshot() {}

impl SkillRegistry {
    pub(crate) async fn load_verified_skill(
        root: PathBuf,
        manifest_bytes: &[u8],
        package_id: &crate::skill_package::SkillPackageId,
        expected_content_hash: String,
        limits: PackageLimits,
        execution_binding: Option<VerifiedExecutionBinding>,
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
        return match binding {
            VerifiedExecutionBinding::Managed(binding) => binding
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
                .map(Some),
            VerifiedExecutionBinding::Bundle(binding) => {
                let prepared = crate::skill_store_execution::prepare_bundle_execution(
                    binding,
                    &verification.expected_content_hash,
                    verification.limits,
                    &skill.manifest,
                )
                .await?;
                checkpoint_bundle_execution_after_snapshot().await;
                Ok(Some(prepared))
            }
        };
    }
    let current = secure_package_hash(&skill.root, verification.limits).await?;
    anyhow::ensure!(
        current == verification.expected_content_hash,
        "verified builtin skill content hash mismatch"
    );
    crate::skill::validate_manifest(&skill.root, &skill.manifest).await?;
    Ok(None)
}
