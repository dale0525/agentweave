use super::{
    OwnerSkillManagementService, SkillManagementError, SkillRollbackOutcome, SkillRollbackReport,
    is_exact_managed_candidate,
};
use crate::events::RuntimeEvent;
use crate::skill_package::{SkillPackageDescriptor, SkillPackageId};
use crate::skill_policy::{ActorContext, SkillOperation};
use crate::skill_source::{ManagedSkillSource, SkillLayer};
use crate::skill_state::{
    NewSkillApproval, SkillApprovalRecord, SkillInstallStatus, SkillLayerRecord,
    SkillRevisionStatus,
};
use crate::skill_state_lifecycle::{ExactLifecyclePublication, LifecycleApproval, LifecycleTarget};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RemovalApprovalBinding {
    package_id: SkillPackageId,
    revision_id: String,
    content_hash: String,
    snapshot_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RollbackApprovalBinding {
    package_id: SkillPackageId,
    target_revision_id: String,
    current_revision_id: String,
    content_hash: String,
    version: String,
    storage_path: String,
    descriptor_document: Value,
    validation_document: Value,
    snapshot_generation: u64,
}

struct RollbackResolution {
    approval_id: String,
    approver_id: String,
    expected_binding: Value,
}

struct UnavailablePublication<'a> {
    actor: &'a ActorContext,
    operation: &'static str,
    package_id: &'a SkillPackageId,
    installation: &'a crate::skill_state::SkillInstallationRecord,
    target: LifecycleTarget<'a>,
    approval: Option<LifecycleApproval<'a>>,
    publication: &'a crate::skill_manager::SkillPublicationGuard,
    candidate: std::sync::Arc<crate::skill_snapshot::SkillSnapshot>,
}

impl OwnerSkillManagementService {
    pub async fn approve_pending_skill_operation(
        &self,
        approval_id: &str,
        actor: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_skill_operation", "skill approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill approval",
            })?;
        match approval.operation.as_str() {
            "activate" => self.approve_activation(approval_id, actor).await,
            "remove" => self.approve_removal(approval_id, actor).await,
            "rollback" => self.approve_rollback(approval_id, actor).await,
            _ => Err(SkillManagementError::Conflict {
                resource: "skill approval",
            }
            .into()),
        }
    }

    pub async fn reject_pending_skill_operation(
        &self,
        approval_id: &str,
        actor: &ActorContext,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_skill_operation", "skill approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill approval",
            })?;
        match approval.operation.as_str() {
            "activate" => self.reject_activation(approval_id, actor).await,
            "remove" => self.reject_removal(approval_id, actor).await,
            "rollback" => self.reject_rollback(approval_id, actor).await,
            _ => Err(SkillManagementError::Conflict {
                resource: "skill approval",
            }
            .into()),
        }
    }

    pub async fn rollback_managed_skill(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        revision_id: &str,
    ) -> anyhow::Result<SkillRollbackOutcome> {
        let service = self.clone();
        let actor = actor.clone();
        let package_id = package_id.clone();
        let revision_id = revision_id.to_string();
        tokio::spawn(async move {
            service
                .rollback_managed_skill_inner(&actor, &package_id, &revision_id)
                .await
        })
        .await
        .map_err(|error| SkillManagementError::internal("rollback", anyhow::Error::new(error)))?
    }

    async fn rollback_managed_skill_inner(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        revision_id: &str,
    ) -> anyhow::Result<SkillRollbackOutcome> {
        self.rollback_managed_skill_resolved(actor, package_id, revision_id, None)
            .await
    }

    async fn rollback_managed_skill_resolved(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        revision_id: &str,
        resolution: Option<RollbackResolution>,
    ) -> anyhow::Result<SkillRollbackOutcome> {
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("rollback", error))?;
        self.deny_builtin(package_id, SkillOperation::Rollback)?;
        self.deny_protected(package_id, SkillOperation::Rollback)?;
        let installation = self
            .active_managed_installation(package_id, "rollback")
            .await?;
        let replaced_revision_id = installation
            .active_revision_id
            .clone()
            .expect("active installation invariant");
        if replaced_revision_id == revision_id {
            return Err(SkillManagementError::Conflict {
                resource: "rollback revision",
            }
            .into());
        }
        let target = self
            .managed_revision(package_id, revision_id, "rollback")
            .await?;
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(target.descriptor_json.clone())?;
        self.authorize(actor, SkillOperation::Rollback, descriptor.kind)?;
        ensure_validated_revision(&target.validation_json, &target.content_hash)?;
        let binding = RollbackApprovalBinding {
            package_id: package_id.clone(),
            target_revision_id: revision_id.into(),
            current_revision_id: replaced_revision_id.clone(),
            content_hash: target.content_hash.clone(),
            version: target.version.clone(),
            storage_path: target.storage_path.clone(),
            descriptor_document: target.descriptor_json.clone(),
            validation_document: target.validation_json.clone(),
            snapshot_generation: publication.base_generation(),
        };
        if let Some(resolution) = &resolution {
            let expected: RollbackApprovalBinding =
                serde_json::from_value(resolution.expected_binding.clone()).map_err(|_| {
                    SkillManagementError::Conflict {
                        resource: "rollback approval",
                    }
                })?;
            if expected != binding {
                return Err(SkillManagementError::Conflict {
                    resource: "rollback approval",
                }
                .into());
            }
        } else if self.policy.rollback_approval_required {
            let approval = self
                .state
                .create_lifecycle_approval_unique(
                    NewSkillApproval {
                        package_id: package_id.clone(),
                        revision_id: revision_id.into(),
                        operation: "rollback".into(),
                        requested_by: actor.actor_id.clone(),
                        permission_diff: json!({}),
                        binding: Some(serde_json::to_value(&binding)?),
                    },
                    "rollback",
                )
                .await
                .map_err(|error| {
                    SkillManagementError::from_state("rollback", "rollback approval", error)
                })?;
            let _ = self.events.send(RuntimeEvent::SkillApprovalRequired {
                approval_id: approval.approval_id.clone(),
                operation: SkillOperation::Rollback,
                package_id: package_id.as_str().into(),
                revision_id: revision_id.into(),
                permission_diff: json!({}),
            });
            return Ok(SkillRollbackOutcome::ApprovalRequired(approval));
        }

        let source_view = publication
            .inspect_sources()
            .await
            .map_err(|error| SkillManagementError::internal("rollback", error))?;
        source_view
            .verify_managed_bindings()
            .await
            .map_err(|error| SkillManagementError::internal("rollback", error))?;
        let candidate_package = ManagedSkillSource::from_store(self.revisions.clone())
            .load_managed_revision(package_id, revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("rollback", "rollback revision", error)
            })?;
        let candidate = publication
            .build_candidate(&source_view, candidate_package)
            .await
            .map_err(|error| SkillManagementError::internal("rollback", error))?;
        verify_snapshot_bindings(&candidate).await?;
        if !candidate.packages().iter().any(|resolved| {
            is_exact_managed_candidate(resolved, package_id, revision_id, &target.content_hash)
        }) {
            return Err(SkillManagementError::Conflict {
                resource: "rollback publication",
            }
            .into());
        }

        self.state
            .commit_exact_lifecycle_publication(ExactLifecyclePublication {
                actor_id: &actor.actor_id,
                operation: "rollback_managed_skill",
                package_id,
                expected_installation: &installation,
                target: LifecycleTarget::Rollback { revision_id },
                approval: resolution.as_ref().map(|approval| LifecycleApproval {
                    approval_id: &approval.approval_id,
                    approver_id: &approval.approver_id,
                    operation: "rollback",
                    expected_binding: &approval.expected_binding,
                }),
                previous_generation: publication.base_generation(),
                previous_members: crate::skill_recovery::snapshot_members(
                    &publication.base_snapshot(),
                ),
                generation: candidate.generation(),
                members: crate::skill_recovery::snapshot_members(&candidate),
            })
            .await
            .map_err(|error| {
                SkillManagementError::from_state("rollback", "rollback publication", error)
            })?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::LifecycleAfterDurableCommit)
            .await;
        let report = publication.publish(candidate);
        let _ = self.events.send(RuntimeEvent::SkillRevisionRolledBack {
            package_id: package_id.as_str().into(),
            revision_id: revision_id.into(),
            generation: report.active_generation,
        });
        let _ = self.events.send(RuntimeEvent::SkillSnapshotPublished {
            generation: report.active_generation,
        });
        Ok(SkillRollbackOutcome::Published(SkillRollbackReport {
            package_id: package_id.clone(),
            active_revision_id: revision_id.into(),
            replaced_revision_id,
            generation: report.active_generation,
        }))
    }

    async fn approve_rollback(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let service = self.clone();
        let approval_id = approval_id.to_string();
        let approver = approver.clone();
        tokio::spawn(async move {
            service
                .approve_rollback_inner(&approval_id, &approver)
                .await
        })
        .await
        .map_err(|error| {
            SkillManagementError::internal("approve_rollback", anyhow::Error::new(error))
        })?
    }

    async fn approve_rollback_inner(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_rollback", "rollback approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "rollback approval",
            })?;
        if approval.operation != "rollback"
            || approval.status != crate::skill_state::SkillApprovalStatus::Pending
            || approval.requested_by == approver.actor_id
        {
            return Err(SkillManagementError::Conflict {
                resource: "rollback approval",
            }
            .into());
        }
        self.deny_builtin(&approval.package_id, SkillOperation::Rollback)?;
        self.deny_protected(&approval.package_id, SkillOperation::Rollback)?;
        let binding = self
            .state
            .approval_binding_value(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_rollback", "rollback approval", error)
            })?;
        let outcome = self
            .rollback_managed_skill_resolved(
                approver,
                &approval.package_id,
                &approval.revision_id,
                Some(RollbackResolution {
                    approval_id: approval_id.into(),
                    approver_id: approver.actor_id.clone(),
                    expected_binding: binding,
                }),
            )
            .await?;
        let SkillRollbackOutcome::Published(report) = outcome else {
            return Err(SkillManagementError::Conflict {
                resource: "rollback approval",
            }
            .into());
        };
        let snapshot = self.manager.current_snapshot();
        Ok(crate::skill_manager::SkillReloadReport {
            previous_generation: report.generation.saturating_sub(1),
            active_generation: report.generation,
            active_packages: snapshot.packages().len(),
            inactive_packages: snapshot.inactive().len(),
        })
    }

    async fn reject_rollback(
        &self,
        approval_id: &str,
        actor: &ActorContext,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval =
            self.state
                .get_approval(approval_id)
                .await?
                .ok_or(SkillManagementError::NotFound {
                    resource: "rollback approval",
                })?;
        if approval.operation != "rollback" {
            return Err(SkillManagementError::Conflict {
                resource: "rollback approval",
            }
            .into());
        }
        self.deny_builtin(&approval.package_id, SkillOperation::Rollback)?;
        self.deny_protected(&approval.package_id, SkillOperation::Rollback)?;
        self.authorize_any_kind(actor, SkillOperation::Rollback)?;
        self.state
            .reject(approval_id, &actor.actor_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_rollback", "rollback approval", error)
            })
            .map_err(Into::into)
    }

    pub async fn disable_managed_skill(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let service = self.clone();
        let actor = actor.clone();
        let package_id = package_id.clone();
        tokio::spawn(async move {
            service
                .disable_managed_skill_inner(&actor, &package_id)
                .await
        })
        .await
        .map_err(|error| SkillManagementError::internal("disable", anyhow::Error::new(error)))?
    }

    async fn disable_managed_skill_inner(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("disable", error))?;
        self.deny_builtin(package_id, SkillOperation::Disable)?;
        self.deny_protected(package_id, SkillOperation::Disable)?;
        let installation = self
            .active_managed_installation(package_id, "disable")
            .await?;
        let revision_id = installation
            .active_revision_id
            .as_deref()
            .expect("active installation invariant");
        let revision = self
            .managed_revision(package_id, revision_id, "disable")
            .await?;
        let descriptor: SkillPackageDescriptor = serde_json::from_value(revision.descriptor_json)?;
        self.authorize(actor, SkillOperation::Disable, descriptor.kind)?;
        let source_view = publication
            .inspect_sources()
            .await
            .map_err(|error| SkillManagementError::internal("disable", error))?;
        source_view
            .verify_managed_bindings()
            .await
            .map_err(|error| SkillManagementError::internal("disable", error))?;
        let candidate = publication
            .build_without_managed(&source_view, package_id)
            .await
            .map_err(|error| SkillManagementError::internal("disable", error))?;
        self.commit_unavailable_publication(UnavailablePublication {
            actor,
            operation: "disable_managed_skill",
            package_id,
            installation: &installation,
            target: LifecycleTarget::Disabled,
            approval: None,
            publication: &publication,
            candidate: candidate.clone(),
        })
        .await?;
        let report = publication.publish(candidate);
        let _ = self.events.send(RuntimeEvent::SkillSnapshotPublished {
            generation: report.active_generation,
        });
        Ok(report)
    }

    pub async fn request_removal(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let service = self.clone();
        let actor = actor.clone();
        let package_id = package_id.clone();
        tokio::spawn(async move { service.request_removal_inner(&actor, &package_id).await })
            .await
            .map_err(|error| {
                SkillManagementError::internal("request_removal", anyhow::Error::new(error))
            })?
    }

    async fn request_removal_inner(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("request_removal", error))?;
        self.deny_builtin(package_id, SkillOperation::DeleteManaged)?;
        self.deny_protected(package_id, SkillOperation::DeleteManaged)?;
        let installation = self
            .removable_managed_installation(package_id, "request_removal")
            .await?;
        let revision_id = installation
            .active_revision_id
            .as_deref()
            .expect("active installation invariant");
        let revision = self
            .managed_revision(package_id, revision_id, "request_removal")
            .await?;
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(revision.descriptor_json.clone())?;
        self.authorize(actor, SkillOperation::DeleteManaged, descriptor.kind)?;
        let binding = RemovalApprovalBinding {
            package_id: package_id.clone(),
            revision_id: revision_id.into(),
            content_hash: revision.content_hash,
            snapshot_generation: publication.base_generation(),
        };
        let approval = self
            .state
            .create_removal_approval_unique(NewSkillApproval {
                package_id: package_id.clone(),
                revision_id: revision_id.into(),
                operation: "remove".into(),
                requested_by: actor.actor_id.clone(),
                permission_diff: json!({}),
                binding: Some(serde_json::to_value(&binding)?),
            })
            .await
            .map_err(|error| {
                SkillManagementError::from_state("request_removal", "removal approval", error)
            })?;
        let _ = self.events.send(RuntimeEvent::SkillApprovalRequired {
            approval_id: approval.approval_id.clone(),
            operation: SkillOperation::DeleteManaged,
            package_id: package_id.as_str().into(),
            revision_id: revision_id.into(),
            permission_diff: json!({}),
        });
        Ok(approval)
    }

    pub async fn approve_removal(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let service = self.clone();
        let approval_id = approval_id.to_string();
        let approver = approver.clone();
        tokio::spawn(async move { service.approve_removal_inner(&approval_id, &approver).await })
            .await
            .map_err(|error| {
                SkillManagementError::internal("approve_removal", anyhow::Error::new(error))
            })?
    }

    async fn approve_removal_inner(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("approve_removal", error))?;
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_removal", "removal approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "removal approval",
            })?;
        if approval.operation != "remove" {
            return Err(SkillManagementError::Conflict {
                resource: "removal approval",
            }
            .into());
        }
        self.deny_builtin(&approval.package_id, SkillOperation::DeleteManaged)?;
        self.deny_protected(&approval.package_id, SkillOperation::DeleteManaged)?;
        let binding_value = self
            .state
            .approval_binding_value(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_removal", "removal approval", error)
            })?;
        let binding: RemovalApprovalBinding = serde_json::from_value(binding_value.clone())?;
        let installation = self
            .removable_managed_installation(&binding.package_id, "approve_removal")
            .await?;
        let revision = self
            .managed_revision(&binding.package_id, &binding.revision_id, "approve_removal")
            .await?;
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(revision.descriptor_json.clone())?;
        self.authorize(approver, SkillOperation::DeleteManaged, descriptor.kind)?;
        if publication.base_generation() != binding.snapshot_generation
            || revision.content_hash != binding.content_hash
            || installation.active_revision_id.as_deref() != Some(&binding.revision_id)
        {
            return Err(SkillManagementError::Conflict {
                resource: "removal approval",
            }
            .into());
        }
        let source_view = publication
            .inspect_sources()
            .await
            .map_err(|error| SkillManagementError::internal("approve_removal", error))?;
        source_view
            .verify_managed_bindings()
            .await
            .map_err(|error| SkillManagementError::internal("approve_removal", error))?;
        let candidate = publication
            .build_without_managed(&source_view, &binding.package_id)
            .await
            .map_err(|error| SkillManagementError::internal("approve_removal", error))?;
        self.commit_unavailable_publication(UnavailablePublication {
            actor: approver,
            operation: "remove_managed_skill",
            package_id: &binding.package_id,
            installation: &installation,
            target: LifecycleTarget::Removed,
            approval: Some(LifecycleApproval {
                approval_id,
                approver_id: &approver.actor_id,
                operation: "remove",
                expected_binding: &binding_value,
            }),
            publication: &publication,
            candidate: candidate.clone(),
        })
        .await?;
        let report = publication.publish(candidate);
        let _ = self.events.send(RuntimeEvent::SkillSnapshotPublished {
            generation: report.active_generation,
        });
        Ok(report)
    }

    pub async fn reject_removal(
        &self,
        approval_id: &str,
        actor: &ActorContext,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval =
            self.state
                .get_approval(approval_id)
                .await?
                .ok_or(SkillManagementError::NotFound {
                    resource: "removal approval",
                })?;
        if approval.operation != "remove" {
            return Err(SkillManagementError::Conflict {
                resource: "removal approval",
            }
            .into());
        }
        let revision = self
            .managed_revision(
                &approval.package_id,
                &approval.revision_id,
                "reject_removal",
            )
            .await?;
        let descriptor: SkillPackageDescriptor = serde_json::from_value(revision.descriptor_json)?;
        self.authorize(actor, SkillOperation::DeleteManaged, descriptor.kind)?;
        self.state
            .reject(approval_id, &actor.actor_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_removal", "removal approval", error)
            })
            .map_err(Into::into)
    }

    async fn commit_unavailable_publication(
        &self,
        input: UnavailablePublication<'_>,
    ) -> anyhow::Result<()> {
        self.state
            .commit_exact_lifecycle_publication(ExactLifecyclePublication {
                actor_id: &input.actor.actor_id,
                operation: input.operation,
                package_id: input.package_id,
                expected_installation: input.installation,
                target: input.target,
                approval: input.approval,
                previous_generation: input.publication.base_generation(),
                previous_members: crate::skill_recovery::snapshot_members(
                    &input.publication.base_snapshot(),
                ),
                generation: input.candidate.generation(),
                members: crate::skill_recovery::snapshot_members(&input.candidate),
            })
            .await
            .map_err(|error| {
                SkillManagementError::from_state(input.operation, "lifecycle publication", error)
            })?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::LifecycleAfterDurableCommit)
            .await;
        Ok(())
    }

    async fn active_managed_installation(
        &self,
        package_id: &SkillPackageId,
        operation: &'static str,
    ) -> anyhow::Result<crate::skill_state::SkillInstallationRecord> {
        let installation = self
            .state
            .get_installation(package_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state(operation, "managed installation", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "managed installation",
            })?;
        if installation.source_layer != SkillLayerRecord::Managed
            || installation.status != SkillInstallStatus::Active
            || !installation.enabled
        {
            return Err(SkillManagementError::Conflict {
                resource: "managed installation",
            }
            .into());
        }
        Ok(installation)
    }

    async fn removable_managed_installation(
        &self,
        package_id: &SkillPackageId,
        operation: &'static str,
    ) -> anyhow::Result<crate::skill_state::SkillInstallationRecord> {
        let installation = self
            .state
            .get_installation(package_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state(operation, "managed installation", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "managed installation",
            })?;
        let removable_status = matches!(
            installation.status,
            SkillInstallStatus::Active | SkillInstallStatus::Disabled
        );
        if installation.source_layer != SkillLayerRecord::Managed
            || !removable_status
            || installation.active_revision_id.is_none()
        {
            return Err(SkillManagementError::Conflict {
                resource: "managed installation",
            }
            .into());
        }
        Ok(installation)
    }

    async fn managed_revision(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        operation: &'static str,
    ) -> anyhow::Result<crate::skill_state::SkillRevisionRecord> {
        let revision = self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state(operation, "managed revision", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "managed revision",
            })?;
        if revision.package_id != *package_id || revision.status != SkillRevisionStatus::Managed {
            return Err(SkillManagementError::Conflict {
                resource: "managed revision",
            }
            .into());
        }
        Ok(revision)
    }

    fn deny_protected(
        &self,
        package_id: &SkillPackageId,
        operation: SkillOperation,
    ) -> Result<(), SkillManagementError> {
        if self.policy.protected_packages.contains(package_id) {
            return Err(SkillManagementError::Denied {
                operation: operation.as_str(),
            });
        }
        Ok(())
    }

    fn deny_builtin(
        &self,
        package_id: &SkillPackageId,
        operation: SkillOperation,
    ) -> Result<(), SkillManagementError> {
        let snapshot = self.manager.current_snapshot();
        let builtin = snapshot
            .packages()
            .iter()
            .chain(snapshot.inactive())
            .any(|resolved| {
                resolved.package.layer == SkillLayer::Builtin
                    && resolved.package.descriptor.id == *package_id
            });
        if builtin {
            return Err(SkillManagementError::Denied {
                operation: operation.as_str(),
            });
        }
        Ok(())
    }
}

fn ensure_validated_revision(validation: &Value, content_hash: &str) -> anyhow::Result<()> {
    let validation = serde_json::from_value::<super::SkillDraftValidation>(validation.clone())
        .map_err(|_| SkillManagementError::Conflict {
            resource: "rollback validation",
        })?;
    if !validation.ok || validation.content_hash != content_hash {
        return Err(SkillManagementError::Conflict {
            resource: "rollback validation",
        }
        .into());
    }
    Ok(())
}

async fn verify_snapshot_bindings(
    snapshot: &crate::skill_snapshot::SkillSnapshot,
) -> anyhow::Result<()> {
    for resolved in snapshot.packages() {
        if resolved.package.layer != SkillLayer::Managed {
            continue;
        }
        let binding = resolved
            .package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .ok_or_else(|| SkillManagementError::Conflict {
                resource: "managed publication binding",
            })?;
        binding
            .store
            .verify_managed_binding(
                &binding.package_id,
                &binding.revision_id,
                &binding.storage_path,
                &resolved.package.content_hash,
            )
            .await?;
    }
    Ok(())
}
