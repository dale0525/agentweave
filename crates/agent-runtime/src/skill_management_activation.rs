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
        self.authorize_any_kind(actor, SkillOperation::Activate)?;
        if self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| SkillManagementError::internal("request_activation", error))?
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
            .await?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Activate, descriptor.kind)?;
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone()).map_err(|_| {
                SkillManagementError::InvalidRequest(
                    "draft must have a persisted validation before activation".into(),
                )
            })?;
        if !validation.ok
            || validation.content_hash != candidate.content_hash
            || validation.snapshot_generation != self.manager.current_snapshot().generation()
            || validation.resolver_status != "active"
        {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation",
            }
            .into());
        }
        let current_snapshot = self.manager.current_snapshot();
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
            content_hash: candidate.content_hash,
            descriptor_document: candidate.record.descriptor_json.clone(),
            validation_digest: json_digest(&validation_document)?,
            validation_document,
            validation_snapshot_generation: validation.snapshot_generation,
            permission_diff_digest: json_digest(&permission_diff)?,
            requesting_actor: actor.actor_id.clone(),
        };
        let (approval, created) = self
            .state
            .create_activation_approval_unique(NewSkillApproval {
                package_id: descriptor.id.clone(),
                revision_id: revision_id.to_string(),
                operation: SkillOperation::Activate.as_str().into(),
                requested_by: actor.actor_id.clone(),
                permission_diff,
                binding: Some(serde_json::to_value(binding)?),
            })
            .await?;
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
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?
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
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?
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
        let collides_with_builtin = publication
            .has_builtin(&binding.package_id)
            .await
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?;
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
            .map_err(|error| -> anyhow::Error {
                let message = error.to_string();
                if message.contains("stale") || message.contains("already exists") {
                    SkillManagementError::Conflict {
                        resource: "activation approval",
                    }
                    .into()
                } else {
                    SkillManagementError::internal("approve_activation", error).into()
                }
            })?;
        let candidate_snapshot = match publication.build_candidate(prepared.candidate()).await {
            Ok(candidate) => candidate,
            Err(error) => {
                let cleanup = prepared.abort().await;
                return Err(combine_activation_error(error, cleanup));
            }
        };
        let exact_active = candidate_snapshot.packages().iter().any(|resolved| {
            resolved.package.layer == crate::skill_source::SkillLayer::Managed
                && resolved.package.descriptor.id == binding.package_id
                && resolved.package.content_hash == binding.content_hash
                && resolved
                    .package
                    .verified_content
                    .as_ref()
                    .and_then(|content| content.execution_binding.as_ref())
                    .is_some_and(|execution| execution.revision_id == binding.revision_id)
        });
        if !exact_active {
            let cleanup = prepared.abort().await;
            return Err(combine_activation_error(
                anyhow::anyhow!("exact activation candidate is inactive"),
                cleanup,
            ));
        }
        let previous = self
            .state
            .get_installation(&binding.package_id)
            .await
            .map_err(|error| SkillManagementError::internal("approve_activation", error))?;
        let members = snapshot_members(&candidate_snapshot);
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
        let report = publication.publish(candidate_snapshot);
        let _ = self.events.send(RuntimeEvent::SkillSnapshotPublished {
            generation: report.active_generation,
        });
        prepared.finish().await;
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
            .map_err(|error| SkillManagementError::internal("reject_activation", error))?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill approval",
            })?;
        let record = self
            .state
            .get_revision(&approval.revision_id)
            .await
            .map_err(|error| SkillManagementError::internal("reject_activation", error))?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill revision",
            })?;
        let descriptor: SkillPackageDescriptor = serde_json::from_value(record.descriptor_json)?;
        self.authorize(actor, SkillOperation::Activate, descriptor.kind)?;
        self.state
            .reject(approval_id, &actor.actor_id)
            .await
            .map_err(|error| {
                if error.to_string().contains("already resolved") {
                    SkillManagementError::Conflict {
                        resource: "skill approval",
                    }
                    .into()
                } else {
                    SkillManagementError::internal("reject_activation", error).into()
                }
            })
    }
}

fn json_digest(value: &Value) -> anyhow::Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn combine_activation_error(error: anyhow::Error, cleanup: anyhow::Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => {
            let message = error.to_string();
            if message.contains("inactive")
                || message.contains("changed")
                || message.contains("conflict")
                || message.contains("stale")
            {
                SkillManagementError::Conflict {
                    resource: "activation publication",
                }
                .into()
            } else {
                SkillManagementError::internal("approve_activation", error).into()
            }
        }
        Err(cleanup) => SkillManagementError::internal(
            "approve_activation",
            error.context(format!("activation compensation failed: {cleanup}")),
        )
        .into(),
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
