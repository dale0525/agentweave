use super::{OwnerSkillManagementService, SkillDraftSummary, SkillManagementError};
use crate::skill_package::{SkillPackageDescriptor, SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillOperation};
use crate::skill_state::SkillInstallStatus;
use serde_json::Value;
use std::path::{Path, PathBuf};

impl OwnerSkillManagementService {
    pub async fn import_draft(
        &self,
        actor: &ActorContext,
        import_name: &Path,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize_any_kind(actor, SkillOperation::Import)?;
        let service = self.clone();
        let actor = actor.clone();
        let import_name = import_name.to_path_buf();
        tokio::spawn(async move { service.import_draft_inner(&actor, &import_name).await })
            .await
            .map_err(|error| {
                SkillManagementError::internal("import_draft", anyhow::Error::new(error))
            })?
    }

    async fn import_draft_inner(
        &self,
        actor: &ActorContext,
        import_name: &Path,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize_any_kind(actor, SkillOperation::Import)?;
        let roots = self
            .transfer_roots
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("skill transfer roots are not configured"))?;
        let relative = transfer_name(import_name)
            .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
        let inspected = self
            .revisions
            .inspect_transfer_package(&roots.import, &relative)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("import_draft", "skill import source", error)
            })?;
        let descriptor = &inspected.descriptor.descriptor;
        if descriptor.kind == SkillPackageKind::NativeRuntime || inspected.has_runtime_manifest {
            return Err(SkillManagementError::InvalidRequest(
                "native runtime imports are disabled by default".into(),
            )
            .into());
        }
        self.authorize(actor, SkillOperation::Import, descriptor.kind)?;
        let imported = self
            .revisions
            .import_quarantined_revision(
                &roots.import,
                &relative,
                &descriptor.id,
                &inspected.content_hash,
                &actor.actor_id,
            )
            .await
            .map_err(|error| {
                SkillManagementError::from_store("import_draft", "skill import", error)
            })?;
        let imported_descriptor: SkillPackageDescriptor =
            serde_json::from_value(imported.descriptor_json.clone()).map_err(|error| {
                SkillManagementError::internal("import_draft", anyhow::Error::new(error))
            })?;
        Ok(SkillDraftSummary {
            package_id: imported.package_id,
            revision_id: imported.revision_id,
            version: imported.version,
            kind: imported_descriptor.kind,
            validation: imported.validation_json,
            status: imported.status.as_str().into(),
        })
    }

    pub async fn export_managed_skill(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        export_name: &Path,
    ) -> anyhow::Result<PathBuf> {
        self.authorize_any_kind(actor, SkillOperation::Export)?;
        let service = self.clone();
        let actor = actor.clone();
        let package_id = package_id.clone();
        let export_name = export_name.to_path_buf();
        tokio::spawn(async move {
            service
                .export_managed_skill_inner(&actor, &package_id, &export_name)
                .await
        })
        .await
        .map_err(|error| {
            SkillManagementError::internal("export_managed_skill", anyhow::Error::new(error))
        })?
    }

    async fn export_managed_skill_inner(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        export_name: &Path,
    ) -> anyhow::Result<PathBuf> {
        self.authorize_any_kind(actor, SkillOperation::Export)?;
        let roots = self
            .transfer_roots
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("skill transfer roots are not configured"))?;
        let relative = transfer_name(export_name)
            .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
        let installation = self
            .state
            .get_installation(package_id)
            .await
            .map_err(|error| SkillManagementError::internal("export_managed_skill", error))?
            .ok_or(SkillManagementError::NotFound {
                resource: "active managed skill",
            })?;
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed
            || installation.status != SkillInstallStatus::Active
            || !installation.enabled
        {
            return Err(SkillManagementError::Conflict {
                resource: "active managed skill",
            }
            .into());
        }
        let revision_id =
            installation
                .active_revision_id
                .ok_or(SkillManagementError::Conflict {
                    resource: "active managed skill",
                })?;
        let record = self
            .state
            .get_revision(&revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state(
                    "export_managed_skill",
                    "active managed revision",
                    error,
                )
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "active managed revision",
            })?;
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(record.descriptor_json.clone())?;
        self.authorize(actor, SkillOperation::Export, descriptor.kind)?;
        if record.validation_json.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(SkillManagementError::Conflict {
                resource: "active managed revision validation",
            }
            .into());
        }
        Ok(self
            .revisions
            .export_managed_revision(&record, &roots.export, &relative)
            .await
            .map_err(|error| {
                SkillManagementError::from_store(
                    "export_managed_skill",
                    "skill export destination",
                    error,
                )
            })?)
    }
}

fn transfer_name(path: &Path) -> anyhow::Result<PathBuf> {
    crate::skill_source::canonical_relative_path(path)?;
    if path.components().count() != 1 {
        anyhow::bail!("skill transfer name must be one relative UTF-8 component");
    }
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("skill transfer name must be UTF-8"))?;
    Ok(path.to_path_buf())
}
