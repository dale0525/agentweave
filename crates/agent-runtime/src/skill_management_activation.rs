use super::{
    OwnerSkillManagementService, SkillApprovalBinding, SkillDraftValidation, SkillManagementError,
};
use crate::events::RuntimeEvent;
use crate::skill_package::SkillPackageDescriptor;
use crate::skill_policy::{ActorContext, SkillOperation};
use crate::skill_state::{NewSkillApproval, SkillApprovalRecord, SkillApprovalStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

impl OwnerSkillManagementService {
    pub async fn request_activation(
        &self,
        actor: &ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let service = self.clone();
        let actor = actor.clone();
        let revision_id = revision_id.to_string();
        tokio::spawn(async move { service.request_activation_inner(&actor, &revision_id).await })
            .await
            .map_err(|error| {
                SkillManagementError::internal("request_activation", anyhow::Error::new(error))
            })?
    }

    async fn request_activation_inner(
        &self,
        actor: &ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.authorize_any_kind(actor, SkillOperation::Activate)?;
        if self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("request_activation", "skill revision", error)
            })?
            .is_none()
        {
            return Err(SkillManagementError::NotFound {
                resource: "skill revision",
            }
            .into());
        }
        let candidate = self
            .revisions
            .snapshot_staging_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("request_activation", "skill revision", error)
            })?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Activate, descriptor.kind)?;
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone()).map_err(|_| {
                SkillManagementError::InvalidRequest(
                    "draft must have a persisted validation before activation".into(),
                )
            })?;
        if !validation.ok
            || validation.revision_id != candidate.record.revision_id
            || validation.content_hash != candidate.content_hash
            || validation.snapshot_generation != self.manager.current_snapshot().generation()
            || validation.resolver_status != "active"
        {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation",
            }
            .into());
        }
        let initial_expectation =
            crate::skill_state::SkillRevisionExpectation::from(&candidate.record);
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationRequestBeforeCommit)
            .await;
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("request_activation", error))?;
        if publication.base_generation() != validation.snapshot_generation {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation generation",
            }
            .into());
        }
        let locked = self
            .revisions
            .lock_staging_revision_snapshot(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("request_activation", "skill revision", error)
            })?;
        let candidate = &locked.snapshot;
        if crate::skill_state::SkillRevisionExpectation::from(&candidate.record)
            != initial_expectation
            || candidate.content_hash != validation.content_hash
        {
            return Err(SkillManagementError::Conflict {
                resource: "skill revision",
            }
            .into());
        }
        let descriptor = &candidate.descriptor.descriptor;
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone()).map_err(|_| {
                SkillManagementError::InvalidRequest(
                    "draft must have a persisted validation before activation".into(),
                )
            })?;
        if !validation.ok
            || validation.revision_id != candidate.record.revision_id
            || validation.content_hash != candidate.content_hash
            || validation.snapshot_generation != publication.base_generation()
            || validation.resolver_status != "active"
        {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation",
            }
            .into());
        }
        let current_snapshot = publication.base_snapshot();
        let collides_with_builtin = current_snapshot
            .packages()
            .iter()
            .chain(current_snapshot.inactive())
            .any(|resolved| {
                resolved.package.layer == crate::skill_source::SkillLayer::Builtin
                    && resolved.package.descriptor.id == descriptor.id
            });
        if (collides_with_builtin || self.policy.protected_packages.contains(&descriptor.id))
            && !self.policy.can_override(actor, &descriptor.id)
        {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::OverrideBuiltin.as_str(),
            }
            .into());
        }
        let permission_diff = validation.permission_diff.clone();
        let validation_document = candidate.record.validation_json.clone();
        let binding = SkillApprovalBinding {
            package_id: descriptor.id.clone(),
            revision_id: revision_id.to_string(),
            revision_version: candidate.record.version.clone(),
            revision_storage_path: candidate.record.storage_path.clone(),
            content_hash: candidate.content_hash.clone(),
            descriptor_document: candidate.record.descriptor_json.clone(),
            validation_digest: json_digest(&validation_document)?,
            validation_document,
            validation_snapshot_generation: validation.snapshot_generation,
            permission_diff_digest: json_digest(&permission_diff)?,
            requesting_actor: actor.actor_id.clone(),
        };
        let revision_expectation =
            crate::skill_state::SkillRevisionExpectation::from(&candidate.record);
        let (approval, created) = self
            .state
            .create_activation_approval_unique(
                crate::skill_state_activation::ExactActivationApprovalRequest {
                    approval: NewSkillApproval {
                        package_id: descriptor.id.clone(),
                        revision_id: revision_id.to_string(),
                        operation: SkillOperation::Activate.as_str().into(),
                        requested_by: actor.actor_id.clone(),
                        permission_diff,
                        binding: Some(serde_json::to_value(binding)?),
                    },
                    revision_expectation,
                    expected_generation: publication.base_generation(),
                },
            )
            .await
            .map_err(|error| {
                SkillManagementError::from_state("request_activation", "skill approval", error)
            })?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationRequestAfterCommit)
            .await;
        if created {
            let _ = self.events.send(RuntimeEvent::SkillApprovalRequired {
                approval_id: approval.approval_id.clone(),
                operation: SkillOperation::Activate,
                package_id: approval.package_id.as_str().to_string(),
                revision_id: approval.revision_id.clone(),
                permission_diff: approval.permission_diff.clone(),
            });
        }
        Ok(approval)
    }

    pub async fn approve_activation(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        let service = self.clone();
        let approval_id = approval_id.to_string();
        let approver = approver.clone();
        tokio::spawn(async move {
            service
                .approve_activation_inner(&approval_id, &approver)
                .await
        })
        .await
        .map_err(|error| {
            SkillManagementError::internal("approve_activation", anyhow::Error::new(error))
        })?
    }

    async fn approve_activation_inner(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        self.authorize_any_kind(approver, SkillOperation::Activate)?;
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?;
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("approve_activation", "skill approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill approval",
            })?;
        if approval.status != SkillApprovalStatus::Pending {
            return Err(SkillManagementError::Conflict {
                resource: "skill approval",
            }
            .into());
        }
        if approval.requested_by == approver.actor_id {
            return Err(SkillManagementError::Conflict {
                resource: "skill approval principal",
            }
            .into());
        }
        let binding_value = self
            .state
            .activation_approval_binding(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state(
                    "approve_activation",
                    "activation approval binding",
                    error,
                )
            })?
            .ok_or(SkillManagementError::Conflict {
                resource: "activation approval binding",
            })?;
        let binding: SkillApprovalBinding =
            serde_json::from_value(binding_value.clone()).map_err(|error| {
                SkillManagementError::internal("approve_activation", anyhow::Error::new(error))
            })?;
        if binding.package_id != approval.package_id
            || binding.revision_id != approval.revision_id
            || binding.requesting_actor != approval.requested_by
            || binding.validation_digest != json_digest(&binding.validation_document)?
            || binding.permission_diff_digest != json_digest(&approval.permission_diff)?
            || binding.validation_snapshot_generation != publication.base_generation()
        {
            return Err(SkillManagementError::Conflict {
                resource: "activation approval binding",
            }
            .into());
        }
        let validation: SkillDraftValidation =
            serde_json::from_value(binding.validation_document.clone()).map_err(|error| {
                SkillManagementError::internal("approve_activation", anyhow::Error::new(error))
            })?;
        if !validation.ok
            || validation.revision_id != binding.revision_id
            || validation.content_hash != binding.content_hash
            || validation.snapshot_generation != binding.validation_snapshot_generation
            || validation.resolver_status != "active"
        {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation",
            }
            .into());
        }
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(binding.descriptor_document.clone()).map_err(|error| {
                SkillManagementError::internal("approve_activation", anyhow::Error::new(error))
            })?;
        self.authorize(approver, SkillOperation::Activate, descriptor.kind)?;
        let source_view = publication
            .inspect_sources()
            .await
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?;
        let collides_with_builtin = source_view.has_builtin(&binding.package_id);
        if (collides_with_builtin || self.policy.protected_packages.contains(&binding.package_id))
            && !self.policy.can_override(approver, &binding.package_id)
        {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::OverrideBuiltin.as_str(),
            }
            .into());
        }
        let expectation = crate::skill_state::SkillRevisionExpectation {
            version: binding.revision_version.clone(),
            content_hash: binding.content_hash.clone(),
            storage_path: binding.revision_storage_path.clone(),
            status: crate::skill_state::SkillRevisionStatus::Staging,
            descriptor_json: binding.descriptor_document.clone(),
            validation_json: binding.validation_document.clone(),
        };
        let prepared = self
            .revisions
            .prepare_managed_activation(&binding.revision_id, expectation)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("approve_activation", "activation approval", error)
            })?;
        let candidate_result = publication
            .build_candidate(&source_view, prepared.candidate())
            .await;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterCandidateBuild)
            .await;
        let candidate_snapshot = match candidate_result {
            Ok(candidate) => candidate,
            Err(error) => {
                let cleanup = prepared.abort().await;
                self.revisions
                    .checkpoint(
                        crate::skill_store_faults::StoreFaultPoint::ActivationAfterCompensation,
                    )
                    .await;
                return Err(combine_activation_error(error, cleanup));
            }
        };
        if let Err(error) = source_view.verify_managed_bindings().await {
            let cleanup = prepared.abort().await;
            self.revisions
                .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterCompensation)
                .await;
            return Err(combine_activation_error(error, cleanup));
        }
        let exact_active = candidate_snapshot.packages().iter().any(|resolved| {
            super::is_exact_managed_candidate(
                resolved,
                &binding.package_id,
                &binding.revision_id,
                &binding.content_hash,
            )
        });
        if !exact_active {
            let cleanup = prepared.abort().await;
            return match cleanup {
                Ok(()) => Err(SkillManagementError::Conflict {
                    resource: "activation publication",
                }
                .into()),
                Err(cleanup) => Err(SkillManagementError::internal(
                    "approve_activation",
                    cleanup.context("activation compensation failed"),
                )
                .into()),
            };
        }
        let previous = self
            .state
            .get_installation(&binding.package_id)
            .await
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?;
        let members = snapshot_members(&candidate_snapshot);
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationBeforeDurableCommit)
            .await;
        if let Err(error) = prepared.revalidate_destination().await {
            let cleanup = prepared.abort().await;
            self.revisions
                .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterCompensation)
                .await;
            return Err(combine_activation_error(error, cleanup));
        }
        let commit = self
            .state
            .commit_exact_activation_publication(
                crate::skill_state_activation::ExactActivationPublication {
                    approval_id,
                    approver_id: &approver.actor_id,
                    expected_binding: &binding_value,
                    package_id: &binding.package_id,
                    revision_id: &binding.revision_id,
                    expectation: prepared.expectation(),
                    promotion: prepared.promotion(),
                    previous_installation: previous.as_ref(),
                    generation: candidate_snapshot.generation(),
                    members,
                },
            )
            .await;
        if let Err(error) = commit {
            let cleanup = prepared.abort().await;
            return Err(combine_activation_error(error, cleanup));
        }
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterDurableCommit)
            .await;
        let report = publication.publish(candidate_snapshot);
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterMemoryPublish)
            .await;
        let _ = self.events.send(RuntimeEvent::SkillSnapshotPublished {
            generation: report.active_generation,
        });
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterEvent)
            .await;
        prepared.finish().await;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ActivationAfterSourceCleanup)
            .await;
        Ok(report)
    }

    pub async fn reject_activation(
        &self,
        approval_id: &str,
        actor: &ActorContext,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.authorize_any_kind(actor, SkillOperation::Activate)?;
        let approval = self
            .state
            .get_approval(approval_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_activation", "skill approval", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill approval",
            })?;
        let record = self
            .state
            .get_revision(&approval.revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_activation", "skill revision", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill revision",
            })?;
        let descriptor: SkillPackageDescriptor = serde_json::from_value(record.descriptor_json)?;
        self.authorize(actor, SkillOperation::Activate, descriptor.kind)?;
        self.state
            .reject(approval_id, &actor.actor_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("reject_activation", "skill approval", error)
            })
            .map_err(Into::into)
    }
}

fn json_digest(value: &Value) -> anyhow::Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn combine_activation_error(error: anyhow::Error, cleanup: anyhow::Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => {
            match error.downcast::<crate::skill_store_public_types::SkillStoreBoundaryError>() {
                Ok(crate::skill_store_public_types::SkillStoreBoundaryError::Conflict(_)) => {
                    SkillManagementError::Conflict {
                        resource: "activation publication",
                    }
                    .into()
                }
                Ok(crate::skill_store_public_types::SkillStoreBoundaryError::NotFound(_)) => {
                    SkillManagementError::NotFound {
                        resource: "activation publication",
                    }
                    .into()
                }
                Ok(crate::skill_store_public_types::SkillStoreBoundaryError::InvalidInput(_)) => {
                    SkillManagementError::InvalidRequest("invalid activation publication".into())
                        .into()
                }
                Err(error) => combine_activation_state_error(error),
            }
        }
        Err(cleanup) => SkillManagementError::internal(
            "approve_activation",
            error.context(format!("activation compensation failed: {cleanup}")),
        )
        .into(),
    }
}

fn combine_activation_state_error(error: anyhow::Error) -> anyhow::Error {
    match error.downcast::<crate::skill_state::SkillStateBoundaryError>() {
        Ok(crate::skill_state::SkillStateBoundaryError::Conflict(_)) => {
            SkillManagementError::Conflict {
                resource: "activation publication",
            }
            .into()
        }
        Ok(crate::skill_state::SkillStateBoundaryError::NotFound(_)) => {
            SkillManagementError::NotFound {
                resource: "activation publication",
            }
            .into()
        }
        Ok(crate::skill_state::SkillStateBoundaryError::InvalidInput(_)) => {
            SkillManagementError::InvalidRequest("invalid activation publication".into()).into()
        }
        Err(error) => SkillManagementError::internal("approve_activation", error).into(),
    }
}

fn snapshot_members(snapshot: &crate::skill_snapshot::SkillSnapshot) -> Value {
    let mut members = snapshot
        .packages()
        .iter()
        .map(|resolved| {
            json!({
                "contentHash": resolved.package.content_hash,
                "packageId": resolved.package.descriptor.id.as_str(),
                "version": resolved.package.descriptor.version.to_string(),
            })
        })
        .collect::<Vec<_>>();
    members.sort_by(|left, right| left["packageId"].as_str().cmp(&right["packageId"].as_str()));
    Value::Array(members)
}
