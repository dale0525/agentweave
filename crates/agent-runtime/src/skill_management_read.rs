use super::{OwnerSkillManagementService, SkillManagementError, SkillPackageStatus};
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
        let summaries = self
            .list_effective_skills(actor)
            .await?
            .into_iter()
            .chain(self.list_managed_skills(actor).await?)
            .filter(|summary| summary.package_id == *package_id)
            .collect::<Vec<_>>();
        let summary = preferred_summary(&summaries).ok_or(SkillManagementError::NotFound {
            resource: "skill package",
        })?;
        let display_name = records
            .iter()
            .find_map(|record| descriptor(record).ok())
            .map(|descriptor| descriptor.display_name)
            .unwrap_or_else(|| display_name(package_id.as_str()));
        let mut revisions = Vec::with_capacity(records.len());
        for record in records {
            revisions.push(self.revision_detail(record).await?);
        }
        if revisions.is_empty()
            && let Some(revision) = self.resolved_revision_detail(package_id, summary)
        {
            revisions.push(revision?);
        }
        let editable_draft = revisions.iter().find(|revision| revision.editable).cloned();
        Ok(SkillPackageDetail {
            package_id: package_id.clone(),
            display_name: if display_name.is_empty() {
                summary.display_name.clone()
            } else {
                display_name
            },
            version: summary.version.clone(),
            source_layer: summary.source_layer.clone(),
            status: summary.status.clone(),
            reason: summary.reason.clone(),
            active_revision_id: summary.active_revision_id.clone(),
            revisions,
            editable_draft,
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
        }))
    }

    async fn revision_detail(
        &self,
        record: SkillRevisionRecord,
    ) -> anyhow::Result<SkillRevisionDetail> {
        let descriptor = descriptor(&record)?;
        let content = self.revisions.inspect_revision_content(&record).await?;
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
        })
    }
}

fn preferred_summary(summaries: &[SkillPackageStatus]) -> Option<&SkillPackageStatus> {
    summaries
        .iter()
        .find(|summary| summary.source_layer == "managed")
        .or_else(|| summaries.first())
}

fn descriptor(record: &SkillRevisionRecord) -> anyhow::Result<SkillPackageDescriptor> {
    Ok(serde_json::from_value(record.descriptor_json.clone())?)
}

fn display_name(package_id: &str) -> String {
    package_id
        .rsplit('.')
        .next()
        .unwrap_or(package_id)
        .replace('-', " ")
}
