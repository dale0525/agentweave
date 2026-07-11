use crate::events::RuntimeEvent;
use crate::skill_authoring::build_package_draft;
use crate::skill_manager::SkillManager;
use crate::skill_package::{SkillPackageDescriptor, SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillManagementPolicy, SkillOperation};
use crate::skill_resolver::SkillResolutionStatus;
use crate::skill_source::SkillLayer;
use crate::skill_state::{
    NewSkillApproval, SkillApprovalRecord, SkillApprovalStatus, SkillAuditRecord,
    SkillInstallStatus, SkillStateStore,
};
use crate::skill_store::SkillRevisionStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct SkillTransferRoots {
    import: crate::skill_store_locks::StoreRootIdentity,
    export: crate::skill_store_locks::StoreRootIdentity,
}

#[derive(Clone)]
pub struct OwnerSkillManagementService {
    manager: SkillManager,
    revisions: SkillRevisionStore,
    state: SkillStateStore,
    policy: SkillManagementPolicy,
    transfer_roots: Option<SkillTransferRoots>,
    events: Arc<Mutex<Vec<RuntimeEvent>>>,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DraftFileUpdate {
    pub path: PathBuf,
    pub content: String,
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
#[serde(rename_all = "camelCase")]
pub struct SkillDraftValidation {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub required_tools: Vec<String>,
    pub dependencies: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub permission_diff: Value,
    pub content_hash: String,
    pub snapshot_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDraftTestResult {
    pub ok: bool,
    pub error_class: Option<String>,
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
            transfer_roots: None,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_transfer_roots(
        mut self,
        import_root: impl AsRef<std::path::Path>,
        export_root: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        self.transfer_roots = Some(SkillTransferRoots {
            import: crate::skill_store_locks::StoreRootIdentity::capture(
                import_root.as_ref().to_path_buf(),
            )?,
            export: crate::skill_store_locks::StoreRootIdentity::capture(
                export_root.as_ref().to_path_buf(),
            )?,
        });
        Ok(self)
    }

    pub async fn with_prepared_transfer_roots(
        self,
        import_root: impl AsRef<std::path::Path>,
        export_root: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<Self> {
        crate::skill_store_secure_fs::prepare_directory_path(import_root.as_ref()).await?;
        crate::skill_store_secure_fs::prepare_directory_path(export_root.as_ref()).await?;
        self.with_transfer_roots(import_root, export_root)
    }

    pub fn policy(&self) -> &SkillManagementPolicy {
        &self.policy
    }

    pub fn emitted_events(&self) -> Vec<RuntimeEvent> {
        self.events
            .lock()
            .expect("skill management event lock poisoned")
            .clone()
    }

    pub async fn create_draft(
        &self,
        actor: &ActorContext,
        request: CreateSkillDraftRequest,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize(actor, SkillOperation::CreateDraft, request.kind)?;
        let authored = build_package_draft(&request)?;
        self.ensure_required_tools_known(&request.required_tools)?;
        self.revisions
            .validate_authored_input(authored.files())
            .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
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

    pub async fn update_draft(
        &self,
        actor: &ActorContext,
        revision_id: &str,
        files: Vec<DraftFileUpdate>,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize_any_kind(actor, SkillOperation::EditDraft)?;
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill revision not found: {revision_id}"))?;
        let kind = serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
            record.descriptor_json,
        )?
        .kind;
        self.authorize(actor, SkillOperation::EditDraft, kind)?;
        let authored = crate::skill_authoring::validate_draft_updates(files)?;
        self.revisions
            .write_staging_files(revision_id, authored)
            .await?;
        let record = self
            .state
            .get_revision(revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill revision not found: {revision_id}"))?;
        Ok(SkillDraftSummary {
            package_id: record.package_id,
            revision_id: record.revision_id,
            version: record.version,
            kind,
            validation: record.validation_json,
            status: "draft".into(),
        })
    }

    pub async fn validate_draft(
        &self,
        actor: &ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillDraftValidation> {
        self.authorize_any_kind(actor, SkillOperation::Validate)?;
        let runtime_snapshot = self.manager.current_snapshot();
        let candidate = self
            .revisions
            .snapshot_inactive_revision(revision_id)
            .await?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Validate, descriptor.kind)?;

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
        if self.policy.protected_packages.contains(&descriptor.id) {
            errors.push(format!(
                "protected package policy denies activation: {}",
                descriptor.id.as_str()
            ));
        }

        let active_descriptor = runtime_snapshot
            .packages()
            .iter()
            .find(|resolved| resolved.package.descriptor.id == descriptor.id)
            .map(|resolved| &resolved.package.descriptor);
        let permission_diff = permission_diff(descriptor, active_descriptor);
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
            dependencies: descriptor
                .requires
                .packages
                .iter()
                .map(|id| id.as_str().to_string())
                .collect(),
            required_capabilities: sorted_strings(&descriptor.requires.capabilities),
            permission_diff,
            content_hash: candidate.content_hash.clone(),
            snapshot_generation: runtime_snapshot.generation(),
        };
        let validation_json = serde_json::to_value(&validation)?;
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
                    .await?;
            }
            crate::skill_state::SkillRevisionStatus::Quarantined if validation.ok => {
                self.revisions
                    .release_quarantined_revision(candidate.record, validation_json)
                    .await?;
            }
            crate::skill_state::SkillRevisionStatus::Quarantined => {
                self.state
                    .refresh_quarantined_revision_validation_cas(
                        revision_id,
                        crate::skill_state::SkillRevisionExpectation::from(&candidate.record),
                        validation_json,
                    )
                    .await?;
            }
            crate::skill_state::SkillRevisionStatus::Managed => {
                anyhow::bail!("managed revision cannot be validated as a draft")
            }
        }
        Ok(validation)
    }

    pub async fn test_draft(
        &self,
        actor: &ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillDraftTestResult> {
        self.authorize_any_kind(actor, SkillOperation::Test)?;
        let candidate = self
            .revisions
            .snapshot_staging_revision(revision_id)
            .await?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(actor, SkillOperation::Test, descriptor.kind)?;
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone()).map_err(|_| {
                SkillManagementError::InvalidRequest(
                    "draft must have a persisted validation before testing".into(),
                )
            })?;
        if validation.content_hash != candidate.content_hash {
            return Err(
                SkillManagementError::InvalidRequest("draft validation is stale".into()).into(),
            );
        }
        let result = SkillDraftTestResult {
            ok: validation.ok,
            error_class: (!validation.ok).then(|| "validation_failed".into()),
        };
        let mut persisted = serde_json::to_value(&validation)?;
        persisted["test"] = serde_json::to_value(&result)?;
        self.state
            .refresh_staging_revision_metadata_cas(
                revision_id,
                crate::skill_state::SkillRevisionExpectation::from(&candidate.record),
                crate::skill_state::SkillRevisionMetadata {
                    version: candidate.record.version,
                    content_hash: candidate.content_hash,
                    descriptor_json: serde_json::to_value(descriptor)?,
                    validation_json: persisted,
                },
            )
            .await?;
        Ok(result)
    }

    pub async fn import_draft(
        &self,
        actor: &ActorContext,
        import_name: &std::path::Path,
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
            .await?;
        let descriptor = &inspected.descriptor.descriptor;
        if descriptor.kind == SkillPackageKind::NativeRuntime || inspected.has_runtime_manifest {
            return Err(SkillManagementError::InvalidRequest(
                "native runtime imports are disabled by default".into(),
            )
            .into());
        }
        self.authorize(actor, SkillOperation::Import, descriptor.kind)?;
        self.ensure_required_tools_known(&descriptor.requires.runtime_tools)?;
        let imported = self
            .revisions
            .import_quarantined_revision(
                &roots.import,
                &relative,
                &inspected.content_hash,
                &actor.actor_id,
            )
            .await?;
        Ok(SkillDraftSummary {
            package_id: descriptor.id.clone(),
            revision_id: imported.revision_id,
            version: descriptor.version.to_string(),
            kind: descriptor.kind,
            validation: json!({"ok": false, "status": "quarantined"}),
            status: "quarantined".into(),
        })
    }

    pub async fn export_managed_skill(
        &self,
        actor: &ActorContext,
        package_id: &SkillPackageId,
        export_name: &std::path::Path,
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
            .await?
            .ok_or_else(|| anyhow::anyhow!("active managed skill not found"))?;
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed
            || installation.status != SkillInstallStatus::Active
            || !installation.enabled
        {
            anyhow::bail!("skill is not an active managed package");
        }
        let revision_id = installation
            .active_revision_id
            .ok_or_else(|| anyhow::anyhow!("active managed skill has no revision"))?;
        let record = self
            .state
            .get_revision(&revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("active managed revision not found"))?;
        let descriptor: SkillPackageDescriptor =
            serde_json::from_value(record.descriptor_json.clone())?;
        self.authorize(actor, SkillOperation::Export, descriptor.kind)?;
        if record.validation_json.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!("active managed revision is not verified");
        }
        self.revisions
            .export_managed_revision(&record, &roots.export, &relative)
            .await
    }

    pub async fn request_activation(
        &self,
        actor: &ActorContext,
        revision_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.authorize_any_kind(actor, SkillOperation::Activate)?;
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
        if !validation.ok || validation.content_hash != candidate.content_hash {
            return Err(SkillManagementError::InvalidRequest(
                "draft validation is not successful or is stale".into(),
            )
            .into());
        }
        let mut permission_diff = validation.permission_diff.clone();
        permission_diff["binding"] = json!({
            "contentHash": candidate.content_hash,
            "validation": candidate.record.validation_json,
        });
        let (approval, created) = self
            .state
            .create_activation_approval_unique(NewSkillApproval {
                package_id: descriptor.id.clone(),
                revision_id: revision_id.to_string(),
                operation: SkillOperation::Activate.as_str().into(),
                requested_by: actor.actor_id.clone(),
                permission_diff,
            })
            .await?;
        if created {
            self.events
                .lock()
                .expect("skill management event lock poisoned")
                .push(RuntimeEvent::SkillApprovalRequired {
                    approval_id: approval.approval_id.clone(),
                    operation: SkillOperation::Activate,
                    package_id: approval.package_id.as_str().to_string(),
                    revision_id: approval.revision_id.clone(),
                    permission_diff: validation.permission_diff,
                });
        }
        Ok(approval)
    }

    pub async fn approve_activation(
        &self,
        approval_id: &str,
        approver: &ActorContext,
    ) -> anyhow::Result<crate::skill_manager::SkillReloadReport> {
        self.authorize_any_kind(approver, SkillOperation::Activate)?;
        let approval = self
            .state
            .get_approval(approval_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill approval not found"))?;
        if approval.status != SkillApprovalStatus::Pending {
            anyhow::bail!("skill approval already resolved: {approval_id}");
        }
        if approval.requested_by == approver.actor_id {
            anyhow::bail!("requester cannot approve their own request");
        }
        let candidate = self
            .revisions
            .snapshot_staging_revision(&approval.revision_id)
            .await?;
        let descriptor = &candidate.descriptor.descriptor;
        self.authorize(approver, SkillOperation::Activate, descriptor.kind)?;
        let binding = approval
            .permission_diff
            .get("binding")
            .ok_or_else(|| anyhow::anyhow!("activation approval binding is missing"))?;
        if binding.get("contentHash").and_then(Value::as_str)
            != Some(candidate.content_hash.as_str())
            || binding.get("validation") != Some(&candidate.record.validation_json)
        {
            anyhow::bail!("activation approval is stale");
        }
        let validation: SkillDraftValidation =
            serde_json::from_value(candidate.record.validation_json.clone())?;
        if !validation.ok
            || validation.snapshot_generation != self.manager.current_snapshot().generation()
        {
            anyhow::bail!("activation approval validation is stale");
        }
        self.ensure_required_tools_known(&descriptor.requires.runtime_tools)?;
        if candidate.runtime_manifest.is_some()
            || descriptor.kind == SkillPackageKind::NativeRuntime
        {
            anyhow::bail!("native runtime activation is disabled");
        }
        crate::skill_catalog::SkillCatalog::read_verified_package_entry(
            PathBuf::from("SKILL.md"),
            candidate
                .instructions_file
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("SKILL.md is missing"))?,
        )?;

        let previous = self.state.get_installation(&approval.package_id).await?;
        let promoted = self
            .revisions
            .promote_revision(&approval.revision_id)
            .await?;
        self.state
            .switch_revision_for_publication(&approval.package_id, &promoted.revision_id)
            .await?;
        let state = self.state.clone();
        let approval_id_owned = approval_id.to_string();
        let approver_id = approver.actor_id.clone();
        let package_id = approval.package_id.clone();
        let revision_id = promoted.revision_id.clone();
        let reload = self
            .manager
            .reload_with_pre_publish(|snapshot| async move {
                let members = snapshot_members(&snapshot);
                state
                    .commit_activation_publication(
                        &approval_id_owned,
                        &approver_id,
                        &package_id,
                        &revision_id,
                        snapshot.generation(),
                        members,
                    )
                    .await?;
                Ok(())
            })
            .await;
        let (report, ()) = match reload {
            Ok(value) => value,
            Err(error) => {
                if let Err(compensation) = self
                    .state
                    .restore_installation_after_failed_publication(
                        &approval.package_id,
                        &promoted.revision_id,
                        previous.as_ref(),
                    )
                    .await
                {
                    return Err(error.context(format!(
                        "publication installation compensation failed: {compensation}"
                    )));
                }
                if let Err(rejection) = self
                    .state
                    .reject(approval_id, "system-publication-failed")
                    .await
                {
                    return Err(error.context(format!(
                        "publication approval rejection failed: {rejection}"
                    )));
                }
                return Err(error);
            }
        };
        self.events
            .lock()
            .expect("skill management event lock poisoned")
            .push(RuntimeEvent::SkillSnapshotPublished {
                generation: report.active_generation,
            });
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
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill approval not found"))?;
        let record = self
            .state
            .get_revision(&approval.revision_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("skill revision not found"))?;
        let descriptor: SkillPackageDescriptor = serde_json::from_value(record.descriptor_json)?;
        self.authorize(actor, SkillOperation::Activate, descriptor.kind)?;
        self.state.reject(approval_id, &actor.actor_id).await
    }

    pub async fn list_effective_skills(
        &self,
        actor: &ActorContext,
    ) -> anyhow::Result<Vec<SkillPackageStatus>> {
        self.authorize_inspect(actor)?;
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
                active_revision_id: resolved_revision_id(resolved),
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
                        active_revision_id: resolved_revision_id(resolved),
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
        for row in self
            .state
            .list_managed_installations_with_revisions()
            .await?
        {
            let installation = row.installation;
            let version = match (&installation.active_revision_id, row.active_version) {
                (Some(_), Some(version)) => version,
                (None, None) => String::new(),
                _ => anyhow::bail!(
                    "managed installation consistency error for {}: active revision version mismatch",
                    installation.package_id.as_str()
                ),
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

    fn ensure_required_tools_known(&self, required_tools: &[String]) -> anyhow::Result<()> {
        if required_tools.is_empty() {
            return Ok(());
        }
        let active = self
            .manager
            .current_snapshot()
            .registry()
            .tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        if let Some(unknown) = required_tools.iter().find(|tool| !active.contains(*tool)) {
            return Err(SkillManagementError::InvalidRequest(format!(
                "unknown required host tool: {unknown}"
            ))
            .into());
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

    fn authorize_any_kind(
        &self,
        actor: &ActorContext,
        operation: SkillOperation,
    ) -> Result<(), SkillManagementError> {
        if self
            .policy
            .allowed_kinds
            .iter()
            .copied()
            .any(|kind| self.policy.allows(actor, operation, kind))
        {
            return Ok(());
        }
        Err(SkillManagementError::Denied {
            operation: operation.as_str(),
        })
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
) -> Value {
    let active_tools: BTreeSet<String> = active
        .map(|descriptor| descriptor.requires.runtime_tools.iter().cloned().collect())
        .unwrap_or_default();
    let active_capabilities: BTreeSet<String> = active
        .map(|descriptor| descriptor.requires.capabilities.iter().cloned().collect())
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
    json!({
        "addedCapabilities": sorted_strings(&added_capabilities),
        "addedTools": sorted_strings(&added_tools),
    })
}

fn transfer_name(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    crate::skill_source::canonical_relative_path(path)?;
    if path.components().count() != 1 {
        anyhow::bail!("skill transfer name must be one relative UTF-8 component");
    }
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("skill transfer name must be UTF-8"))?;
    Ok(path.to_path_buf())
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

fn layer_name(layer: SkillLayer) -> &'static str {
    match layer {
        SkillLayer::Builtin => "builtin",
        SkillLayer::Managed => "managed",
        SkillLayer::Session => "session",
    }
}

fn resolved_revision_id(resolved: &crate::skill_resolver::ResolvedSkillPackage) -> Option<String> {
    resolved
        .package
        .verified_content
        .as_ref()?
        .execution_binding
        .as_ref()
        .map(|binding| binding.revision_id.clone())
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
