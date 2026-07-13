use super::{
    LayeredSkillInventoryItem, OwnerSkillManagementService, SkillActionFacts, SkillManagementError,
    SkillPackageStatus,
};
use crate::skill_package::{SkillPackageDescriptor, SkillPackageId, SkillPackageKind};
use crate::skill_policy::ActorContext;
use crate::skill_state::{SkillRevisionRecord, SkillRevisionStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillRevisionRequirements {
    pub runtime_tools: Vec<String>,
    pub capabilities: Vec<String>,
    pub connectors: Vec<String>,
    pub packages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SkillRevisionDetail {
    pub revision_id: String,
    pub version: String,
    pub status: String,
    pub editable: bool,
    pub created_by: String,
    pub created_at: String,
    pub kind: SkillPackageKind,
    pub instructions: String,
    pub validation: Value,
    pub requirements: SkillRevisionRequirements,
    pub permission_diff: Value,
    pub content_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SkillPackageDetail {
    pub package_id: SkillPackageId,
    pub display_name: String,
    pub version: String,
    pub source_layer: String,
    pub status: String,
    pub reason: String,
    pub active_revision_id: Option<String>,
    pub effective: Option<SkillPackageStatus>,
    pub managed: Option<SkillPackageStatus>,
    pub built_in_collision: bool,
    pub actions: SkillActionFacts,
    pub revisions: Vec<SkillRevisionDetail>,
    pub editable_draft: Option<SkillRevisionDetail>,
}

impl OwnerSkillManagementService {
    pub async fn get_skill_detail(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<SkillPackageDetail> {
        self.authorize_inspect(actor)?;
        let records = self.state.list_package_revisions(package_id).await?;
        let layered = self
            .list_layered_skills(actor)
            .await?
            .into_iter()
            .find(|item| item.package_id == *package_id)
            .ok_or(SkillManagementError::NotFound {
                resource: "skill package",
            })?;
        let summary = layered
            .effective
            .as_ref()
            .or(layered.managed.as_ref())
            .expect("layered inventory item must contain a layer");
        let mut revisions = Vec::with_capacity(records.len());
        for record in records {
            revisions.push(self.revision_detail(record).await?);
        }
        if let Some(effective) = layered.effective.as_ref()
            && !revisions.iter().any(|revision| {
                Some(revision.revision_id.as_str()) == effective.active_revision_id.as_deref()
            })
            && let Some(revision) = self.resolved_revision_detail(package_id, effective)
        {
            revisions.push(revision?);
        }
        let editable_draft = revisions.iter().find(|revision| revision.editable).cloned();
        Ok(SkillPackageDetail {
            package_id: package_id.clone(),
            display_name: summary.display_name.clone(),
            version: summary.version.clone(),
            source_layer: summary.source_layer.clone(),
            status: summary.status.clone(),
            reason: summary.reason.clone(),
            active_revision_id: summary.active_revision_id.clone(),
            effective: layered.effective,
            managed: layered.managed,
            built_in_collision: layered.built_in_collision,
            actions: layered.actions,
            revisions,
            editable_draft,
        })
    }

    pub async fn list_layered_skills(
        &self,
        actor: &ActorContext,
    ) -> anyhow::Result<Vec<LayeredSkillInventoryItem>> {
        let effective_rows = self.list_effective_skills(actor).await?;
        let managed_rows = self
            .list_managed_skills(actor)
            .await?
            .into_iter()
            .filter(|status| status.status != "removed")
            .collect::<Vec<_>>();
        let mut package_ids = effective_rows
            .iter()
            .chain(&managed_rows)
            .map(|status| status.package_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut inventory = Vec::with_capacity(package_ids.len());
        for package_id in std::mem::take(&mut package_ids) {
            let effective = effective_rows
                .iter()
                .find(|status| status.package_id == package_id && status.available)
                .cloned();
            let managed = managed_rows
                .iter()
                .find(|status| status.package_id == package_id)
                .cloned();
            let has_builtin = effective_rows
                .iter()
                .any(|status| status.package_id == package_id && status.source_layer == "builtin");
            let has_managed = managed.is_some()
                || effective_rows.iter().any(|status| {
                    status.package_id == package_id && status.source_layer == "managed"
                });
            let built_in_collision = has_builtin && has_managed;
            let actions = self
                .skill_action_facts(
                    actor,
                    &package_id,
                    effective.as_ref(),
                    managed.as_ref(),
                    has_builtin,
                )
                .await?;
            inventory.push(LayeredSkillInventoryItem {
                package_id,
                effective,
                managed,
                built_in_collision,
                actions,
            });
        }
        Ok(inventory)
    }

    async fn skill_action_facts(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        effective: Option<&SkillPackageStatus>,
        managed: Option<&SkillPackageStatus>,
        has_builtin: bool,
    ) -> anyhow::Result<SkillActionFacts> {
        if self.policy.protected_packages.contains(package_id) {
            return Ok(SkillActionFacts::default());
        }
        let records = self.state.list_package_revisions(package_id).await?;
        let draft = records
            .iter()
            .find(|record| record.status == SkillRevisionStatus::Staging);
        let active = managed
            .and_then(|status| status.active_revision_id.as_deref())
            .and_then(|revision_id| {
                records
                    .iter()
                    .find(|record| record.revision_id == revision_id)
            });
        let descriptor = draft.or(active).and_then(|record| descriptor(record).ok());
        let Some(descriptor) = descriptor else {
            return Ok(SkillActionFacts::default());
        };
        let active_managed = effective.is_some_and(|status| status.source_layer == "managed")
            && managed.is_some_and(|status| status.status == "active");
        let validated_draft = draft.is_some_and(|record| {
            record
                .validation_json
                .get("ok")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        let override_allowed = !has_builtin || self.policy.can_override(actor, package_id);
        let managed_revision_count = records
            .iter()
            .filter(|record| record.status == SkillRevisionStatus::Managed)
            .count();
        Ok(SkillActionFacts {
            can_edit_draft: draft.is_some()
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::EditDraft,
                    descriptor.kind,
                ),
            can_validate_draft: draft.is_some()
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::Validate,
                    descriptor.kind,
                ),
            can_request_activation: validated_draft
                && override_allowed
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::Activate,
                    descriptor.kind,
                ),
            can_disable: active_managed
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::Disable,
                    descriptor.kind,
                ),
            can_request_removal: managed.is_some_and(|status| status.status != "removed")
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::DeleteManaged,
                    descriptor.kind,
                ),
            can_rollback: active_managed
                && managed_revision_count > 1
                && self.policy.allows(
                    actor,
                    crate::skill_policy::SkillOperation::Rollback,
                    descriptor.kind,
                ),
        })
    }

    fn resolved_revision_detail(
        &self,
        package_id: &SkillPackageId,
        summary: &SkillPackageStatus,
    ) -> Option<anyhow::Result<SkillRevisionDetail>> {
        let snapshot = self.manager.current_snapshot();
        let resolved = snapshot
            .packages()
            .iter()
            .chain(snapshot.inactive())
            .find(|resolved| resolved.package.descriptor.id == *package_id)?;
        let descriptor = &resolved.package.descriptor;
        let instructions = resolved
            .package
            .verified_content
            .as_ref()
            .and_then(|content| content.instructions_file.as_ref())
            .map(|bytes| String::from_utf8(bytes.to_vec()))
            .transpose()
            .map_err(anyhow::Error::from);
        Some(instructions.map(|instructions| SkillRevisionDetail {
            revision_id: summary.active_revision_id.clone().unwrap_or_else(|| {
                format!("{}:{}", summary.source_layer, resolved.package.content_hash)
            }),
            version: summary.version.clone(),
            status: summary.status.clone(),
            editable: false,
            created_by: "system".into(),
            created_at: String::new(),
            kind: descriptor.kind,
            instructions: instructions.unwrap_or_default(),
            validation: json!({
                "ok": summary.status == "active",
                "errors": if summary.status == "active" { Vec::<String>::new() } else { vec![summary.reason.clone()] },
                "warnings": []
            }),
            requirements: SkillRevisionRequirements {
                runtime_tools: descriptor.requires.runtime_tools.clone(),
                capabilities: descriptor.requires.capabilities.clone(),
                connectors: descriptor.requires.connectors.clone(),
                packages: descriptor.requires.packages.iter().map(|item| item.as_str().to_string()).collect(),
            },
            permission_diff: json!({}),
            content_hash: resolved.package.content_hash.clone(),
        }))
    }

    async fn revision_detail(
        &self,
        record: SkillRevisionRecord,
    ) -> anyhow::Result<SkillRevisionDetail> {
        let descriptor = descriptor(&record)?;
        let content = self
            .revisions
            .inspect_revision_content(&record)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("inspect skill revision", "skill revision", error)
            })?;
        let requirements = SkillRevisionRequirements {
            runtime_tools: descriptor.requires.runtime_tools.clone(),
            capabilities: descriptor.requires.capabilities.clone(),
            connectors: descriptor.requires.connectors.clone(),
            packages: descriptor
                .requires
                .packages
                .iter()
                .map(|package| package.as_str().to_string())
                .collect(),
        };
        let permission_diff = record
            .validation_json
            .get("permissionDiff")
            .cloned()
            .unwrap_or_else(|| json!({}));
        Ok(SkillRevisionDetail {
            revision_id: record.revision_id,
            version: record.version,
            status: record.status.as_str().into(),
            editable: record.status == SkillRevisionStatus::Staging,
            created_by: record.created_by,
            created_at: record.created_at.to_rfc3339(),
            kind: descriptor.kind,
            instructions: content.instructions,
            validation: record.validation_json,
            requirements,
            permission_diff,
            content_hash: record.content_hash,
        })
    }
}

fn descriptor(record: &SkillRevisionRecord) -> anyhow::Result<SkillPackageDescriptor> {
    Ok(serde_json::from_value(record.descriptor_json.clone())?)
}
