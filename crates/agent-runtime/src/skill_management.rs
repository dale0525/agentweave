use crate::skill_authoring::build_package_draft;
use crate::skill_manager::SkillManager;
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillManagementPolicy, SkillOperation};
use crate::skill_resolver::SkillResolutionStatus;
use crate::skill_source::SkillLayer;
use crate::skill_state::{SkillAuditRecord, SkillInstallStatus, SkillLayerRecord, SkillStateStore};
use crate::skill_store::SkillRevisionStore;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct OwnerSkillManagementService {
    manager: SkillManager,
    revisions: SkillRevisionStore,
    state: SkillStateStore,
    policy: SkillManagementPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillDraftRequest {
    pub package_id: SkillPackageId,
    pub display_name: String,
    pub description: String,
    pub kind: SkillPackageKind,
    #[serde(default)]
    pub required_tools: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SkillDraftSummary {
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub version: String,
    pub kind: SkillPackageKind,
    pub validation: serde_json::Value,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillPackageStatus {
    pub package_id: SkillPackageId,
    pub version: String,
    pub source_layer: String,
    pub status: String,
    pub reason: String,
    pub active_revision_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillManagementError {
    #[error("skills.{operation} denied")]
    Denied { operation: &'static str },
    #[error("{0}")]
    InvalidRequest(String),
}

impl OwnerSkillManagementService {
    pub fn new(
        manager: SkillManager,
        revisions: SkillRevisionStore,
        state: SkillStateStore,
        policy: SkillManagementPolicy,
    ) -> Self {
        Self {
            manager,
            revisions,
            state,
            policy,
        }
    }

    pub fn policy(&self) -> &SkillManagementPolicy {
        &self.policy
    }

    pub async fn create_draft(
        &self,
        actor: &ActorContext,
        request: CreateSkillDraftRequest,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize(actor, SkillOperation::CreateDraft, request.kind)?;
        let authored = build_package_draft(&request)?;
        let revision = self
            .revisions
            .create_authored_staging_revision(
                &request.package_id,
                request.kind,
                authored.files(),
                &actor.actor_id,
            )
            .await?;
        Ok(SkillDraftSummary {
            package_id: request.package_id,
            revision_id: revision.revision_id,
            version: "0.1.0".into(),
            kind: request.kind,
            validation: serde_json::json!({"status": "pending"}),
            status: "draft".into(),
        })
    }

    pub async fn list_effective_skills(
        &self,
        actor: &ActorContext,
    ) -> anyhow::Result<Vec<SkillPackageStatus>> {
        self.authorize_inspect(actor)?;
        let active_installations = self.state.list_active_installations().await?;
        let active_revisions = active_installations
            .into_iter()
            .filter_map(|installation| {
                installation
                    .active_revision_id
                    .map(|revision| (installation.package_id, revision))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let snapshot = self.manager.current_snapshot();
        let mut statuses = snapshot
            .packages()
            .iter()
            .map(|resolved| SkillPackageStatus {
                package_id: resolved.package.descriptor.id.clone(),
                version: resolved.package.descriptor.version.to_string(),
                source_layer: layer_name(resolved.package.layer).into(),
                status: "active".into(),
                reason: resolved.reason.clone(),
                active_revision_id: resolved_revision_id(resolved, &active_revisions),
            })
            .chain(
                snapshot
                    .inactive()
                    .iter()
                    .map(|resolved| SkillPackageStatus {
                        package_id: resolved.package.descriptor.id.clone(),
                        version: resolved.package.descriptor.version.to_string(),
                        source_layer: layer_name(resolved.package.layer).into(),
                        status: resolution_status_name(resolved.status).into(),
                        reason: resolved.reason.clone(),
                        active_revision_id: resolved_revision_id(resolved, &active_revisions),
                    }),
            )
            .collect::<Vec<_>>();
        sort_statuses(&mut statuses);
        Ok(statuses)
    }

    pub async fn list_managed_skills(
        &self,
        actor: &ActorContext,
    ) -> anyhow::Result<Vec<SkillPackageStatus>> {
        self.authorize_inspect(actor)?;
        let mut statuses = Vec::new();
        for installation in self.state.list_installations().await? {
            if installation.source_layer != SkillLayerRecord::Managed {
                continue;
            }
            let version = match installation.active_revision_id.as_deref() {
                Some(revision_id) => self
                    .state
                    .get_revision(revision_id)
                    .await?
                    .map(|revision| revision.version)
                    .unwrap_or_default(),
                None => String::new(),
            };
            statuses.push(SkillPackageStatus {
                package_id: installation.package_id,
                version,
                source_layer: "managed".into(),
                status: installation.status.as_str().into(),
                reason: installation_reason(installation.status, installation.enabled).into(),
                active_revision_id: installation.active_revision_id,
            });
        }
        sort_statuses(&mut statuses);
        Ok(statuses)
    }

    pub async fn list_audit(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Vec<SkillAuditRecord>> {
        self.authorize_inspect(actor)?;
        self.state.list_audit(package_id).await
    }

    fn authorize(
        &self,
        actor: &ActorContext,
        operation: SkillOperation,
        kind: SkillPackageKind,
    ) -> Result<(), SkillManagementError> {
        if !self.policy.allows(actor, operation, kind) {
            return Err(SkillManagementError::Denied {
                operation: operation.as_str(),
            });
        }
        Ok(())
    }

    fn authorize_inspect(&self, actor: &ActorContext) -> Result<(), SkillManagementError> {
        if !self.policy.can_inspect(actor) {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::Inspect.as_str(),
            });
        }
        Ok(())
    }
}

fn layer_name(layer: SkillLayer) -> &'static str {
    match layer {
        SkillLayer::Builtin => "builtin",
        SkillLayer::Managed => "managed",
        SkillLayer::Session => "session",
    }
}

fn resolved_revision_id(
    resolved: &crate::skill_resolver::ResolvedSkillPackage,
    active_revisions: &std::collections::BTreeMap<SkillPackageId, String>,
) -> Option<String> {
    (resolved.package.layer == SkillLayer::Managed)
        .then(|| {
            active_revisions
                .get(&resolved.package.descriptor.id)
                .cloned()
        })
        .flatten()
}

fn resolution_status_name(status: SkillResolutionStatus) -> &'static str {
    match status {
        SkillResolutionStatus::Active => "active",
        SkillResolutionStatus::Overridden => "overridden",
        SkillResolutionStatus::OverrideDenied => "override_denied",
        SkillResolutionStatus::ProtectedPackage => "protected_package",
        SkillResolutionStatus::DependencyMissing => "dependency_missing",
        SkillResolutionStatus::CapabilityMissing => "capability_missing",
        SkillResolutionStatus::PlatformUnsupported => "platform_unsupported",
        SkillResolutionStatus::RuntimeIncompatible => "runtime_incompatible",
    }
}

fn installation_reason(status: SkillInstallStatus, enabled: bool) -> &'static str {
    if !enabled {
        "disabled by installation state"
    } else {
        match status {
            SkillInstallStatus::Active => "active",
            SkillInstallStatus::Disabled => "disabled",
            SkillInstallStatus::Inactive => "inactive",
            SkillInstallStatus::Quarantined => "quarantined",
            SkillInstallStatus::Removed => "removed",
        }
    }
}

fn sort_statuses(statuses: &mut [SkillPackageStatus]) {
    statuses.sort_by(|left, right| {
        left.package_id
            .cmp(&right.package_id)
            .then_with(|| left.source_layer.cmp(&right.source_layer))
            .then_with(|| left.status.cmp(&right.status))
    });
}
