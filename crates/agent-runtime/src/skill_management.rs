use crate::events::RuntimeEvent;
use crate::skill_authoring::build_package_draft;
use crate::skill_manager::SkillManager;
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillManagementPolicy, SkillOperation};
use crate::skill_resolver::SkillResolutionStatus;
use crate::skill_source::SkillLayer;
use crate::skill_state::{SkillAuditRecord, SkillInstallStatus, SkillStateStore};
use crate::skill_store::SkillRevisionStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

#[path = "skill_management_activation.rs"]
mod activation;
#[path = "skill_management_lifecycle.rs"]
mod lifecycle;
#[path = "skill_management_read.rs"]
mod read;
#[path = "skill_management_transfer.rs"]
mod transfer;
#[path = "skill_management_validation.rs"]
mod validation;

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
    events: broadcast::Sender<RuntimeEvent>,
    connector_catalog: Arc<BTreeSet<String>>,
    draft_test_deadline: std::time::Duration,
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
    pub required_connectors: Vec<String>,
    pub dependencies: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub resolver_status: String,
    pub resolver_errors: Vec<String>,
    pub permission_diff: Value,
    pub revision_id: String,
    pub content_hash: String,
    pub snapshot_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillDraftTestResult {
    pub ok: bool,
    pub error_class: Option<String>,
    pub content_hash: String,
    pub snapshot_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillApprovalBinding {
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub revision_version: String,
    pub revision_storage_path: String,
    pub content_hash: String,
    pub descriptor_document: Value,
    pub validation_digest: String,
    pub validation_document: Value,
    pub validation_snapshot_generation: u64,
    pub permission_diff_digest: String,
    pub requesting_actor: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillPackageStatus {
    pub package_id: SkillPackageId,
    pub display_name: String,
    pub version: String,
    pub source_layer: String,
    pub status: String,
    pub reason: String,
    pub active_revision_id: Option<String>,
    pub available: bool,
    pub content_hash: Option<String>,
    pub manageable: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillActionFacts {
    pub can_edit_draft: bool,
    pub can_validate_draft: bool,
    pub can_request_activation: bool,
    pub can_disable: bool,
    pub can_request_removal: bool,
    pub can_rollback: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LayeredSkillInventoryItem {
    pub package_id: SkillPackageId,
    pub effective: Option<SkillPackageStatus>,
    pub managed: Option<SkillPackageStatus>,
    pub built_in_collision: bool,
    pub actions: SkillActionFacts,
}

pub use read::{SkillPackageDetail, SkillRevisionDetail, SkillRevisionRequirements};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillRollbackReport {
    pub package_id: SkillPackageId,
    pub active_revision_id: String,
    pub replaced_revision_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SkillRollbackOutcome {
    Published(SkillRollbackReport),
    ApprovalRequired(crate::skill_state::SkillApprovalRecord),
}

#[derive(Debug, thiserror::Error)]
pub enum SkillManagementError {
    #[error("skills.{operation} denied")]
    Denied { operation: &'static str },
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{resource} not found")]
    NotFound { resource: &'static str },
    #[error("{resource} conflicts with current state")]
    Conflict { resource: &'static str },
    #[error("skill management operation failed")]
    Internal {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

impl SkillManagementError {
    pub(crate) fn internal(operation: &'static str, source: anyhow::Error) -> Self {
        Self::Internal { operation, source }
    }

    pub(crate) fn from_store(
        operation: &'static str,
        resource: &'static str,
        error: anyhow::Error,
    ) -> Self {
        match error.downcast::<crate::skill_store_public_types::SkillStoreBoundaryError>() {
            Ok(crate::skill_store_public_types::SkillStoreBoundaryError::InvalidInput(_)) => {
                Self::InvalidRequest("invalid skill package input".into())
            }
            Ok(crate::skill_store_public_types::SkillStoreBoundaryError::NotFound(_)) => {
                Self::NotFound { resource }
            }
            Ok(crate::skill_store_public_types::SkillStoreBoundaryError::Conflict(_)) => {
                Self::Conflict { resource }
            }
            Err(error) => Self::from_state(operation, resource, error),
        }
    }

    pub(crate) fn from_state(
        operation: &'static str,
        resource: &'static str,
        error: anyhow::Error,
    ) -> Self {
        match error.downcast::<crate::skill_state::SkillStateBoundaryError>() {
            Ok(crate::skill_state::SkillStateBoundaryError::InvalidInput(_)) => {
                Self::InvalidRequest("invalid skill state request".into())
            }
            Ok(crate::skill_state::SkillStateBoundaryError::NotFound(_)) => {
                Self::NotFound { resource }
            }
            Ok(crate::skill_state::SkillStateBoundaryError::Conflict(_)) => {
                Self::Conflict { resource }
            }
            Err(error) => Self::internal(operation, error),
        }
    }
}

impl OwnerSkillManagementService {
    pub fn new(
        manager: SkillManager,
        revisions: SkillRevisionStore,
        state: SkillStateStore,
        policy: SkillManagementPolicy,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        manager.bind_managed_runtime(revisions.clone(), events.clone());
        Self {
            manager,
            revisions,
            state,
            policy,
            transfer_roots: None,
            events,
            connector_catalog: Arc::new(BTreeSet::new()),
            draft_test_deadline: std::time::Duration::from_secs(2),
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

    pub fn subscribe_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn with_draft_test_deadline(mut self, deadline: std::time::Duration) -> Self {
        self.draft_test_deadline = deadline;
        self
    }

    pub fn with_connector_catalog<I, S>(mut self, connectors: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let values = connectors
            .into_iter()
            .map(|connector| connector.as_ref().to_string())
            .collect::<Vec<_>>();
        let parsed = parse_connector_ids(&values);
        if let Some(error) = parsed.errors.first() {
            return Err(SkillManagementError::InvalidRequest(format!(
                "invalid host connector catalog: {error}"
            ))
            .into());
        }
        self.connector_catalog = Arc::new(parsed.canonical.into_iter().collect());
        Ok(self)
    }

    pub async fn create_draft(
        &self,
        actor: &ActorContext,
        request: CreateSkillDraftRequest,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.create_draft_with_files(actor, request, Vec::new())
            .await
    }

    pub async fn create_draft_with_files(
        &self,
        actor: &ActorContext,
        request: CreateSkillDraftRequest,
        files: Vec<DraftFileUpdate>,
    ) -> anyhow::Result<SkillDraftSummary> {
        self.authorize(actor, SkillOperation::CreateDraft, request.kind)?;
        let generated = build_package_draft(&request)?;
        self.ensure_required_tools_known(&request.required_tools)?;
        let supplied;
        let authored = if files.is_empty() {
            generated.files()
        } else {
            supplied = crate::skill_authoring::validate_draft_updates(files)?;
            &supplied
        };
        self.revisions
            .validate_authored_input(authored)
            .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
        let revision = self
            .revisions
            .create_authored_staging_revision(
                &request.package_id,
                request.kind,
                authored,
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
            .await
            .map_err(|error| {
                SkillManagementError::from_state("update_draft", "skill revision", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill revision",
            })?;
        let kind = serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
            record.descriptor_json,
        )?
        .kind;
        self.authorize(actor, SkillOperation::EditDraft, kind)?;
        let authored = crate::skill_authoring::validate_draft_updates(files)?;
        self.revisions
            .write_staging_files(revision_id, authored)
            .await
            .map_err(|error| {
                SkillManagementError::from_store("update_draft", "skill revision", error)
            })?;
        let record = self
            .state
            .get_revision(revision_id)
            .await
            .map_err(|error| {
                SkillManagementError::from_state("update_draft", "skill revision", error)
            })?
            .ok_or(SkillManagementError::NotFound {
                resource: "skill revision",
            })?;
        Ok(SkillDraftSummary {
            package_id: record.package_id,
            revision_id: record.revision_id,
            version: record.version,
            kind,
            validation: record.validation_json,
            status: "draft".into(),
        })
    }

    pub async fn list_effective_skills(
        &self,
        actor: &ActorContext,
    ) -> anyhow::Result<Vec<SkillPackageStatus>> {
        self.authorize_inspect(actor)?;
        self.manager.converge_to_authoritative_generation().await?;
        let snapshot = self.manager.current_snapshot();
        let mut statuses = snapshot
            .packages()
            .iter()
            .map(|resolved| SkillPackageStatus {
                package_id: resolved.package.descriptor.id.clone(),
                display_name: resolved.package.descriptor.display_name.clone(),
                version: resolved.package.descriptor.version.to_string(),
                source_layer: layer_name(resolved.package.layer).into(),
                status: "active".into(),
                reason: resolved.reason.clone(),
                active_revision_id: inventory_revision_id(resolved),
                available: true,
                content_hash: Some(resolved.package.content_hash.clone()),
                manageable: self.resolved_manageable(actor, resolved),
            })
            .chain(
                snapshot
                    .inactive()
                    .iter()
                    .map(|resolved| SkillPackageStatus {
                        package_id: resolved.package.descriptor.id.clone(),
                        display_name: resolved.package.descriptor.display_name.clone(),
                        version: resolved.package.descriptor.version.to_string(),
                        source_layer: layer_name(resolved.package.layer).into(),
                        status: resolution_status_name(resolved.status).into(),
                        reason: resolved.reason.clone(),
                        active_revision_id: inventory_revision_id(resolved),
                        available: false,
                        content_hash: Some(resolved.package.content_hash.clone()),
                        manageable: self.resolved_manageable(actor, resolved),
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
            let active_content_hash = row.active_content_hash.clone();
            let descriptor =
                match installation.active_revision_id.as_deref() {
                    Some(revision_id) => self.state.get_revision(revision_id).await?.and_then(
                        |record| {
                            serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
                                record.descriptor_json,
                            )
                            .ok()
                        },
                    ),
                    None => None,
                };
            let display_name = descriptor
                .as_ref()
                .map(|descriptor| descriptor.display_name.clone())
                .unwrap_or_else(|| display_name(installation.package_id.as_str()));
            let version = match (&installation.active_revision_id, row.active_version) {
                (Some(_), Some(version)) => version,
                (None, None) => String::new(),
                _ => anyhow::bail!(
                    "managed installation consistency error for {}: active revision version mismatch",
                    installation.package_id.as_str()
                ),
            };
            let manageable = descriptor.as_ref().is_some_and(|descriptor| {
                installation.status.as_str() == "active"
                    && !self
                        .policy
                        .protected_packages
                        .contains(&installation.package_id)
                    && [
                        SkillOperation::Disable,
                        SkillOperation::DeleteManaged,
                        SkillOperation::Rollback,
                    ]
                    .into_iter()
                    .any(|operation| self.policy.allows(actor, operation, descriptor.kind))
            });
            statuses.push(SkillPackageStatus {
                package_id: installation.package_id,
                display_name,
                version,
                source_layer: "managed".into(),
                status: installation.status.as_str().into(),
                reason: installation_reason(installation.status, installation.enabled).into(),
                active_revision_id: installation.active_revision_id,
                available: installation.status.as_str() == "active" && installation.enabled,
                content_hash: active_content_hash,
                manageable,
            });
        }
        for revision in self.state.list_staging_revisions().await? {
            if statuses
                .iter()
                .any(|status| status.package_id == revision.package_id)
            {
                continue;
            }
            let descriptor =
                serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
                    revision.descriptor_json.clone(),
                )
                .ok();
            let manageable = descriptor.as_ref().is_some_and(|descriptor| {
                !self
                    .policy
                    .protected_packages
                    .contains(&revision.package_id)
                    && [
                        SkillOperation::EditDraft,
                        SkillOperation::Validate,
                        SkillOperation::Activate,
                    ]
                    .into_iter()
                    .any(|operation| self.policy.allows(actor, operation, descriptor.kind))
            });
            statuses.push(SkillPackageStatus {
                display_name: descriptor
                    .map(|descriptor| descriptor.display_name)
                    .unwrap_or_else(|| display_name(revision.package_id.as_str())),
                package_id: revision.package_id,
                version: revision.version,
                source_layer: "managed".into(),
                status: "draft".into(),
                reason: "editable staging draft".into(),
                active_revision_id: None,
                available: false,
                content_hash: Some(revision.content_hash),
                manageable,
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

    fn resolved_manageable(
        &self,
        actor: &ActorContext,
        resolved: &crate::skill_resolver::ResolvedSkillPackage,
    ) -> bool {
        resolved.package.layer == SkillLayer::Managed
            && !self
                .policy
                .protected_packages
                .contains(&resolved.package.descriptor.id)
            && [
                SkillOperation::Disable,
                SkillOperation::DeleteManaged,
                SkillOperation::Rollback,
            ]
            .into_iter()
            .any(|operation| {
                self.policy
                    .allows(actor, operation, resolved.package.descriptor.kind)
            })
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

pub(crate) struct ParsedConnectorIds {
    pub(crate) canonical: Vec<String>,
    pub(crate) errors: Vec<&'static str>,
}

pub(crate) fn parse_connector_ids(values: &[String]) -> ParsedConnectorIds {
    let mut canonical = Vec::new();
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            errors.push("connector id must not be empty");
            continue;
        }
        if !is_valid_connector_id(&normalized) {
            errors.push("connector id contains invalid characters");
            continue;
        }
        if !seen.insert(normalized.clone()) {
            errors.push("duplicate connector id after normalization");
        }
        if value != &normalized {
            errors.push("connector id must use canonical lowercase ASCII");
            continue;
        }
        canonical.push(normalized);
    }
    canonical.sort();
    canonical.dedup();
    errors.sort_unstable();
    errors.dedup();
    ParsedConnectorIds { canonical, errors }
}

fn is_valid_connector_id(value: &str) -> bool {
    let mut previous_separator = true;
    for byte in value.bytes() {
        let separator = matches!(byte, b'.' | b'-' | b'_');
        if !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || separator)
            || (separator && previous_separator)
        {
            return false;
        }
        previous_separator = separator;
    }
    !previous_separator
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

fn inventory_revision_id(resolved: &crate::skill_resolver::ResolvedSkillPackage) -> Option<String> {
    resolved_revision_id(resolved).or_else(|| {
        Some(format!(
            "{}:{}",
            layer_name(resolved.package.layer),
            resolved.package.content_hash
        ))
    })
}

pub(crate) fn is_exact_managed_candidate(
    resolved: &crate::skill_resolver::ResolvedSkillPackage,
    package_id: &SkillPackageId,
    revision_id: &str,
    content_hash: &str,
) -> bool {
    resolved.package.layer == SkillLayer::Managed
        && resolved.package.descriptor.id == *package_id
        && resolved.package.content_hash == content_hash
        && resolved_revision_id(resolved).as_deref() == Some(revision_id)
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
        SkillResolutionStatus::CircuitOpen => "circuit_open",
        SkillResolutionStatus::NetworkPolicyUnavailable => "network_policy_unavailable",
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

fn display_name(package_id: &str) -> String {
    package_id
        .rsplit('.')
        .next()
        .unwrap_or(package_id)
        .replace('-', " ")
}
