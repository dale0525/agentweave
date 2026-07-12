use super::{
    OwnerSkillManagementService, SkillDraftTestResult, SkillDraftValidation, SkillManagementError,
};
use crate::skill_package::{SkillPackageDescriptor, SkillPackageKind};
use crate::skill_policy::{SkillGrant, SkillOperation};
use crate::skill_source::{
    DiscoveredSkillPackage, ManagedExecutionBinding, SkillLayer, VerifiedPackageContent,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

struct DraftTestEvaluation {
    candidate: crate::skill_store_draft::StagingPackageSnapshot,
    validation: SkillDraftValidation,
    result: SkillDraftTestResult,
    publication: crate::skill_manager::SkillPublicationGuard,
}

impl OwnerSkillManagementService {
    pub async fn validate_draft(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillDraftValidation> {
        self.authorize_any_kind(actor, SkillOperation::Validate)?;
        if self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("validate_draft", "skill revision", error)
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
            .snapshot_inactive_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("validate_draft", "skill revision", error)
            })?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Validate, descriptor.kind)?;
        if self.policy.protected_packages.contains(&descriptor.id)
            && !self.policy.can_override(actor, &descriptor.id)
        {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::OverrideBuiltin.as_str(),
            }
            .into());
        }

        let preview = self
            .manager
            .preview_candidate(discovered_candidate(
                &candidate,
                self.revisions.clone(),
                self.revisions.store_limits(),
            ))
            .await;
        let runtime_snapshot = preview
            .as_ref()
            .map(|preview| preview.base.clone())
            .unwrap_or_else(|_| self.manager.current_snapshot());
        let collides_with_builtin = preview.as_ref().is_ok_and(|preview| {
            preview
                .candidate
                .packages()
                .iter()
                .chain(preview.candidate.inactive())
                .any(|resolved| {
                    resolved.package.layer == SkillLayer::Builtin
                        && resolved.package.descriptor.id == descriptor.id
                })
        });
        if collides_with_builtin && !self.policy.can_override(actor, &descriptor.id) {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::OverrideBuiltin.as_str(),
            }
            .into());
        }
        if collides_with_builtin && !actor.grants.contains(&SkillGrant::OverrideBuiltin) {
            return Err(SkillManagementError::Denied {
                operation: SkillOperation::OverrideBuiltin.as_str(),
            }
            .into());
        }

        let mut errors = Vec::new();
        if !self.policy.allowed_kinds.contains(&descriptor.kind) {
            errors.push(format!(
                "package kind is not allowed: {:?}",
                descriptor.kind
            ));
        }
        if descriptor.kind == SkillPackageKind::NativeRuntime
            || candidate.runtime_manifest.is_some()
        {
            errors.push("native runtime payloads are disabled for owner-authored skills".into());
        }
        let active_tools = runtime_snapshot
            .registry()
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        for tool in &descriptor.requires.runtime_tools {
            if !active_tools.contains(tool) {
                errors.push(format!("unknown required host tool: {tool}"));
            }
        }
        let parsed_connectors = super::parse_connector_ids(&descriptor.requires.connectors);
        for error in &parsed_connectors.errors {
            errors.push(format!("invalid required connector: {error}"));
        }
        for connector in &parsed_connectors.canonical {
            if !self.connector_catalog.contains(connector) {
                errors.push(format!("unknown required connector: {connector}"));
            }
        }
        match candidate.instructions_file.as_deref() {
            Some(bytes) => {
                if let Err(error) = crate::skill_catalog::SkillCatalog::read_verified_package_entry(
                    PathBuf::from("SKILL.md"),
                    bytes,
                ) {
                    errors.push(format!("catalog parse failed: {error}"));
                }
            }
            None => errors.push("catalog parse failed: SKILL.md is missing".into()),
        }
        let active_ids = runtime_snapshot
            .packages()
            .iter()
            .map(|resolved| resolved.package.descriptor.id.clone())
            .collect::<BTreeSet<_>>();
        for dependency in &descriptor.requires.packages {
            if !active_ids.contains(dependency) && dependency != &descriptor.id {
                errors.push(format!("missing dependency: {}", dependency.as_str()));
            }
        }
        let (platform, capabilities, runtime_version) = self.manager.validation_runtime();
        for capability in &descriptor.requires.capabilities {
            if !capabilities.contains_name(capability) {
                errors.push(format!("missing capability: {capability}"));
            }
        }
        if descriptor
            .compatibility
            .minimum_runtime_version
            .as_ref()
            .is_some_and(|minimum| minimum > &runtime_version)
        {
            errors.push(format!(
                "runtime {runtime_version} is below the package minimum"
            ));
        }
        let platform_name = format!("{platform:?}").to_ascii_lowercase();
        if !descriptor.compatibility.platforms.is_empty()
            && !descriptor
                .compatibility
                .platforms
                .iter()
                .any(|item| item.eq_ignore_ascii_case(&platform_name))
        {
            errors.push(format!("unsupported platform: {platform_name}"));
        }
        let active_descriptor = runtime_snapshot
            .packages()
            .iter()
            .find(|resolved| resolved.package.descriptor.id == descriptor.id)
            .map(|resolved| &resolved.package.descriptor);
        let permission_diff =
            permission_diff(descriptor, active_descriptor, &parsed_connectors.canonical);
        let (resolver_status, resolver_errors) = match preview {
            Ok(preview) => {
                let exact_active = preview.candidate.packages().iter().any(|resolved| {
                    super::is_exact_managed_candidate(
                        resolved,
                        &descriptor.id,
                        &candidate.record.revision_id,
                        &candidate.content_hash,
                    )
                });
                if exact_active {
                    ("active".to_string(), Vec::new())
                } else if let Some(inactive) =
                    preview.candidate.inactive().iter().find(|resolved| {
                        super::is_exact_managed_candidate(
                            resolved,
                            &descriptor.id,
                            &candidate.record.revision_id,
                            &candidate.content_hash,
                        )
                    })
                {
                    let message = format!(
                        "resolver {}: {}",
                        super::resolution_status_name(inactive.status),
                        inactive.reason
                    );
                    errors.push(message.clone());
                    (
                        super::resolution_status_name(inactive.status).to_string(),
                        vec![message],
                    )
                } else {
                    let message = "resolver did not retain the exact managed candidate".to_string();
                    errors.push(message.clone());
                    ("missing".to_string(), vec![message])
                }
            }
            Err(_) => {
                let message = "candidate snapshot build failed".to_string();
                errors.push(message.clone());
                ("build_failed".to_string(), vec![message])
            }
        };
        errors.sort();
        errors.dedup();
        let mut warnings = candidate.descriptor.warnings;
        warnings.sort();
        warnings.dedup();
        let validation = SkillDraftValidation {
            ok: errors.is_empty(),
            errors,
            warnings,
            required_tools: sorted_strings(&descriptor.requires.runtime_tools),
            required_connectors: parsed_connectors.canonical,
            dependencies: descriptor
                .requires
                .packages
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            required_capabilities: sorted_strings(&descriptor.requires.capabilities),
            resolver_status,
            resolver_errors,
            permission_diff,
            revision_id: candidate.record.revision_id.clone(),
            content_hash: candidate.content_hash.clone(),
            snapshot_generation: runtime_snapshot.generation(),
        };
        let validation_json = serde_json::to_value(&validation)?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::ValidateDraftBeforePersist)
            .await;
        match candidate.record.status {
            crate::skill_state::SkillRevisionStatus::Staging => {
                self.state
                    .refresh_staging_revision_metadata_cas(
                        revision_id,
                        crate::skill_state::SkillRevisionExpectation::from(&candidate.record),
                        crate::skill_state::SkillRevisionMetadata {
                            version: candidate.record.version,
                            content_hash: candidate.content_hash,
                            descriptor_json: serde_json::to_value(descriptor)?,
                            validation_json,
                        },
                    )
                    .await
                    .map_err(|error| {
                        SkillManagementError::from_state("validate_draft", "skill revision", error)
                    })?;
            }
            crate::skill_state::SkillRevisionStatus::Quarantined if validation.ok => {
                self.revisions
                    .release_quarantined_revision(candidate.record, validation_json)
                    .await
                    .map_err(|error| {
                        SkillManagementError::from_store("validate_draft", "skill revision", error)
                    })?;
            }
            crate::skill_state::SkillRevisionStatus::Quarantined => {
                self.state
                    .refresh_quarantined_revision_validation_cas(
                        revision_id,
                        crate::skill_state::SkillRevisionExpectation::from(&candidate.record),
                        validation_json,
                    )
                    .await
                    .map_err(|error| {
                        SkillManagementError::from_state("validate_draft", "skill revision", error)
                    })?;
            }
            crate::skill_state::SkillRevisionStatus::Managed => {
                anyhow::bail!("managed revision cannot be validated as a draft")
            }
        }
        Ok(validation)
    }

    pub async fn test_draft(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillDraftTestResult> {
        let deadline = tokio::time::Instant::now() + self.draft_test_deadline;
        let service = self.clone();
        let actor = actor.clone();
        let revision_id = revision_id.to_string();
        tokio::spawn(async move {
            service
                .test_draft_detached(&actor, &revision_id, deadline)
                .await
        })
        .await
        .map_err(|error| SkillManagementError::internal("test_draft", anyhow::Error::new(error)))?
    }

    async fn test_draft_detached(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
        deadline: tokio::time::Instant,
    ) -> anyhow::Result<SkillDraftTestResult> {
        let evaluation =
            match tokio::time::timeout_at(deadline, self.evaluate_draft_test(actor, revision_id))
                .await
            {
                Ok(result) => result?,
                Err(_) => self.timeout_draft_test(actor, revision_id).await?,
            };
        let result = evaluation.result.clone();
        self.persist_draft_test_owned(evaluation).await?;
        Ok(result)
    }

    async fn evaluate_draft_test(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<DraftTestEvaluation> {
        self.authorize_any_kind(actor, SkillOperation::Test)?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::DraftTestBeforeSnapshot)
            .await;
        let (candidate, validation) = self.load_draft_test(actor, revision_id).await?;
        let descriptor = &candidate.descriptor.descriptor;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::DraftTestBeforePreview)
            .await;
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("test_draft", error))?;
        if publication.base_generation() != validation.snapshot_generation {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation generation",
            }
            .into());
        }
        let preview = publication
            .preview_candidate(discovered_candidate(
                &candidate,
                self.revisions.clone(),
                self.revisions.store_limits(),
            ))
            .await;
        let preview_ok = match preview {
            Ok(preview) => {
                let active = preview.candidate.packages().iter().any(|resolved| {
                    super::is_exact_managed_candidate(
                        resolved,
                        &descriptor.id,
                        &candidate.record.revision_id,
                        &candidate.content_hash,
                    )
                });
                active && preview.base.generation() == validation.snapshot_generation
            }
            Err(_) => false,
        };
        let error_class = validation_error_class(&validation, preview_ok);
        Ok(DraftTestEvaluation {
            result: SkillDraftTestResult {
                ok: validation.ok && preview_ok,
                error_class,
                content_hash: candidate.content_hash.clone(),
                snapshot_generation: validation.snapshot_generation,
            },
            candidate,
            validation,
            publication,
        })
    }

    async fn load_draft_test(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<(
        crate::skill_store_draft::StagingPackageSnapshot,
        SkillDraftValidation,
    )> {
        if self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("test_draft", "skill revision", error)
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
                SkillManagementError::from_store("test_draft", "skill revision", error)
            })?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Test, descriptor.kind)?;
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone()).map_err(|_| {
                SkillManagementError::InvalidRequest(
                    "draft must have a persisted validation before testing".into(),
                )
            })?;
        if validation.revision_id != candidate.record.revision_id
            || validation.content_hash != candidate.content_hash
        {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation",
            }
            .into());
        }
        if validation.snapshot_generation != self.manager.current_snapshot().generation() {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation generation",
            }
            .into());
        }
        Ok((candidate, validation))
    }

    async fn timeout_draft_test(
        &self,
        actor: &crate::skill_policy::ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<DraftTestEvaluation> {
        let (candidate, validation) = self.load_draft_test(actor, revision_id).await?;
        let publication = self
            .manager
            .begin_publication()
            .await
            .map_err(|error| SkillManagementError::internal("test_draft", error))?;
        if publication.base_generation() != validation.snapshot_generation {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation generation",
            }
            .into());
        }
        Ok(DraftTestEvaluation {
            result: SkillDraftTestResult {
                ok: false,
                error_class: Some("timeout".into()),
                content_hash: candidate.content_hash.clone(),
                snapshot_generation: validation.snapshot_generation,
            },
            candidate,
            validation,
            publication,
        })
    }

    async fn persist_draft_test_owned(
        &self,
        evaluation: DraftTestEvaluation,
    ) -> anyhow::Result<()> {
        let service = self.clone();
        let task = tokio::spawn(async move { service.persist_draft_test(evaluation).await });
        task.await.map_err(|error| {
            SkillManagementError::internal("test_draft", anyhow::Error::new(error))
        })??;
        Ok(())
    }

    async fn persist_draft_test(&self, evaluation: DraftTestEvaluation) -> anyhow::Result<()> {
        let DraftTestEvaluation {
            candidate,
            validation,
            result,
            publication,
        } = evaluation;
        if publication.base_generation() != validation.snapshot_generation {
            return Err(SkillManagementError::Conflict {
                resource: "draft validation generation",
            }
            .into());
        }
        let mut persisted = serde_json::to_value(&validation)?;
        persisted["test"] = serde_json::to_value(&result)?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::DraftTestBeforePersist)
            .await;
        let descriptor_json = serde_json::to_value(&candidate.descriptor.descriptor)?;
        self.state
            .refresh_staging_revision_metadata_cas(
                &candidate.record.revision_id,
                crate::skill_state::SkillRevisionExpectation::from(&candidate.record),
                crate::skill_state::SkillRevisionMetadata {
                    version: candidate.record.version,
                    content_hash: candidate.content_hash,
                    descriptor_json,
                    validation_json: persisted,
                },
            )
            .await
            .map_err(|error| {
                SkillManagementError::from_state("test_draft", "skill revision", error)
            })?;
        self.revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::DraftTestAfterPersist)
            .await;
        drop(publication);
        Ok(())
    }
}

fn validation_error_class(validation: &SkillDraftValidation, preview_ok: bool) -> Option<String> {
    if validation
        .errors
        .iter()
        .any(|error| error.starts_with("unknown required host tool"))
    {
        Some("unknown_tool".to_string())
    } else if validation
        .errors
        .iter()
        .any(|error| error.starts_with("unknown required connector"))
    {
        Some("unknown_connector".to_string())
    } else if validation
        .errors
        .iter()
        .any(|error| error.starts_with("missing capability"))
    {
        Some("forbidden_capability".to_string())
    } else if !preview_ok {
        Some("resolver_inactive".to_string())
    } else if !validation.ok {
        Some("validation_failed".to_string())
    } else {
        None
    }
}

fn discovered_candidate(
    candidate: &crate::skill_store_draft::StagingPackageSnapshot,
    store: crate::skill_store::SkillRevisionStore,
    limits: crate::skill_store::SkillStoreLimits,
) -> DiscoveredSkillPackage {
    DiscoveredSkillPackage {
        layer: SkillLayer::Managed,
        root: PathBuf::from(&candidate.record.storage_path),
        descriptor: candidate.descriptor.descriptor.clone(),
        content_hash: candidate.content_hash.clone(),
        warnings: candidate.descriptor.warnings.clone(),
        verified_content: Some(VerifiedPackageContent {
            runtime_manifest: candidate.runtime_manifest.clone().map(Into::into),
            instructions_file: candidate.instructions_file.clone().map(Into::into),
            file_paths: Arc::new(BTreeSet::new()),
            expected_content_hash: candidate.content_hash.clone(),
            limits,
            execution_binding: Some(ManagedExecutionBinding {
                store,
                package_id: candidate.record.package_id.clone(),
                revision_id: candidate.record.revision_id.clone(),
                storage_path: PathBuf::from(&candidate.record.storage_path),
            }),
            bundle_execution_binding: None,
        }),
    }
}

fn sorted_strings(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}

fn permission_diff(
    candidate: &SkillPackageDescriptor,
    active: Option<&SkillPackageDescriptor>,
    candidate_connectors: &[String],
) -> Value {
    let active_tools: BTreeSet<String> = active
        .map(|descriptor| descriptor.requires.runtime_tools.iter().cloned().collect())
        .unwrap_or_default();
    let active_capabilities: BTreeSet<String> = active
        .map(|descriptor| descriptor.requires.capabilities.iter().cloned().collect())
        .unwrap_or_default();
    let active_connectors: BTreeSet<String> = active
        .map(|descriptor| descriptor.requires.connectors.iter().cloned().collect())
        .unwrap_or_default();
    let added_tools = candidate
        .requires
        .runtime_tools
        .iter()
        .filter(|item| !active_tools.contains(*item))
        .cloned()
        .collect::<Vec<_>>();
    let added_capabilities = candidate
        .requires
        .capabilities
        .iter()
        .filter(|item| !active_capabilities.contains(*item))
        .cloned()
        .collect::<Vec<_>>();
    let added_connectors = candidate_connectors
        .iter()
        .filter(|item| !active_connectors.contains(*item))
        .cloned()
        .collect::<Vec<_>>();
    let candidate_tools = candidate
        .requires
        .runtime_tools
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidate_capabilities = candidate
        .requires
        .capabilities
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let candidate_connectors = candidate_connectors
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let removed_tools = active_tools
        .difference(&candidate_tools)
        .cloned()
        .collect::<Vec<_>>();
    let removed_capabilities = active_capabilities
        .difference(&candidate_capabilities)
        .cloned()
        .collect::<Vec<_>>();
    let removed_connectors = active_connectors
        .difference(&candidate_connectors)
        .cloned()
        .collect::<Vec<_>>();
    json!({
        "addedCapabilities": sorted_strings(&added_capabilities),
        "addedConnectors": sorted_strings(&added_connectors),
        "addedTools": sorted_strings(&added_tools),
        "removedCapabilities": sorted_strings(&removed_capabilities),
        "removedConnectors": sorted_strings(&removed_connectors),
        "removedTools": sorted_strings(&removed_tools),
    })
}
