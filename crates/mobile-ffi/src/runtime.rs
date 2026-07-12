use crate::types::{
    MobileDiagnostics, MobileInitConfig, MobileMessageDto, MobileModelConfigDto, MobileSessionDto,
    MobileSkillDto, MobileTurnDto,
};
use agent_runtime::mobile_host::{HttpMobileRuntimeHost, MobileRuntimeInit, SecretResolver};
use agent_runtime::model_config::StoredModelConfig;
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_bundle::BundleSkillSource;
use agent_runtime::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService, SkillDraftSummary,
    SkillDraftValidation, SkillPackageStatus, SkillRollbackOutcome,
};
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::SkillPackageId;
use agent_runtime::skill_policy::{
    ActorContext, SkillManagementMode, SkillManagementPolicy, SkillOperation,
};
use agent_runtime::skill_recovery::RecoveryStatus;
use agent_runtime::skill_resolver::SkillResolutionStatus;
use agent_runtime::skill_source::{ManagedSkillSource, SkillLayer, SkillSource};
use agent_runtime::skill_state::{SkillApprovalRecord, SkillStateStore};
use agent_runtime::skill_store::{SkillRevisionStore, SkillStorePaths};
use agent_runtime::storage::Storage;
use agent_runtime::tools::RuntimeConfig;
use anyhow::{Context, Result};
use model_gateway::provider::EndpointType;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

pub struct MobileRuntime {
    tokio: Runtime,
    storage: Storage,
    init: MobileRuntimeInit,
    skill_manager: SkillManager,
    skill_management: OwnerSkillManagementService,
    skill_state: SkillStateStore,
    skill_policy: SkillManagementPolicy,
    actor_context: ActorContext,
    runtime_config: RuntimeConfig,
    quarantine_root: PathBuf,
    database_ready: bool,
    skills_ready: bool,
    last_reload_status: Mutex<String>,
    model_configured: AtomicBool,
    cancellation: CancellationToken,
}

impl MobileRuntime {
    pub fn initialize(config: MobileInitConfig) -> Result<Self> {
        let tokio = Runtime::new()?;
        let platform = parse_platform(&config.platform)?;
        let capabilities = CapabilitySet::from_names(config.capabilities.clone());
        let app_data_dir = prepare_private_root(&config.app_data_dir)?;
        let cache_dir = prepare_private_root(&config.cache_dir)?;
        let allowed_roots = [app_data_dir.clone(), cache_dir.clone()];
        let builtin_skills_path = resolve_private_path(
            &config.builtin_skills_dir,
            &app_data_dir,
            &allowed_roots,
            "built-in skills directory",
        )?;
        let managed_skills_path = resolve_private_path(
            &config.managed_skills_dir,
            &app_data_dir,
            &allowed_roots,
            "managed skills directory",
        )?;
        let staging_skills_path = resolve_private_path(
            &config.staging_skills_dir,
            &cache_dir,
            &allowed_roots,
            "staging skills directory",
        )?;
        let quarantine_skills_path = resolve_private_path(
            &config.quarantine_skills_dir,
            &app_data_dir,
            &allowed_roots,
            "quarantine skills directory",
        )?;
        ensure_distinct_roots(&[
            &builtin_skills_path,
            &managed_skills_path,
            &staging_skills_path,
            &quarantine_skills_path,
        ])?;
        let database_path = resolve_private_path(
            &config.database_path,
            &app_data_dir,
            &allowed_roots,
            "database path",
        )?;
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
        let storage = tokio.block_on(Storage::connect(&database_url))?;
        let init = MobileRuntimeInit {
            platform,
            capabilities,
        };
        let state = SkillStateStore::new(storage.clone());
        let store_paths = tokio.block_on(SkillStorePaths::prepare(&app_data_dir, &cache_dir))?;
        ensure_configured_store_root(
            &managed_skills_path,
            &store_paths.managed,
            "managed skills directory",
        )?;
        ensure_configured_store_root(
            &staging_skills_path,
            &store_paths.staging,
            "staging skills directory",
        )?;
        ensure_configured_store_root(
            &quarantine_skills_path,
            &store_paths.quarantine,
            "quarantine skills directory",
        )?;
        let revisions = SkillRevisionStore::new(store_paths, state.clone());
        let builtin = tokio.block_on(BundleSkillSource::open(&builtin_skills_path))?;
        let managed = ManagedSkillSource::from_store(revisions.clone());
        let sources: Vec<Arc<dyn SkillSource>> = vec![Arc::new(builtin), Arc::new(managed)];
        let skill_manager =
            tokio.block_on(SkillManager::new_deferred_managed(SkillManagerConfig {
                sources,
                platform: init.platform,
                capabilities: init.capabilities.clone(),
                protected_packages: config
                    .skill_policy
                    .protected_packages
                    .iter()
                    .cloned()
                    .collect(),
                allowed_overrides: config
                    .skill_policy
                    .allowed_overrides
                    .iter()
                    .cloned()
                    .collect(),
                runtime_version: env!("CARGO_PKG_VERSION").parse()?,
            }))?;
        let skill_management = OwnerSkillManagementService::new(
            skill_manager.clone(),
            revisions,
            state.clone(),
            config.skill_policy.clone(),
        );
        let recovery = tokio
            .block_on(skill_manager.startup_reconcile())
            .context("managed skill startup reconciliation failed")?;
        let model_configured = tokio.block_on(storage.load_model_config())?.is_some();
        let runtime_config = RuntimeConfig::workspace_write(&app_data_dir, &app_data_dir);

        Ok(Self {
            tokio,
            storage,
            init,
            skill_manager,
            skill_management,
            skill_state: state,
            skill_policy: config.skill_policy,
            actor_context: config.actor_context,
            runtime_config,
            quarantine_root: quarantine_skills_path,
            database_ready: true,
            skills_ready: true,
            last_reload_status: Mutex::new(recovery_status_name(recovery.status).into()),
            model_configured: AtomicBool::new(model_configured),
            cancellation: CancellationToken::new(),
        })
    }

    pub fn diagnostics(&self) -> MobileDiagnostics {
        let snapshot = self.skill_manager.current_snapshot();
        MobileDiagnostics {
            platform: platform_name(self.init.platform).to_string(),
            capabilities: self.init.capabilities.names().to_vec(),
            database_ready: self.database_ready,
            skills_ready: self.skills_ready,
            model_configured: self.model_configured.load(Ordering::Acquire),
            skill_management_mode: management_mode_name(self.skill_policy.mode).into(),
            active_snapshot_generation: snapshot.generation(),
            quarantined_count: count_real_directories(&self.quarantine_root),
            last_reload_status: self
                .last_reload_status
                .lock()
                .map(|status| status.clone())
                .unwrap_or_else(|_| "unavailable".into()),
        }
    }

    pub fn list_skills(&self) -> Vec<MobileSkillDto> {
        let snapshot = self.skill_manager.current_snapshot();
        let active_revisions = self
            .tokio
            .block_on(self.skill_state.list_managed_installations_with_revisions())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                row.installation
                    .active_revision_id
                    .map(|revision| (row.installation.package_id, revision))
            })
            .collect::<BTreeMap<_, _>>();
        let mut inventory = BTreeMap::new();
        for resolved in snapshot.packages().iter().chain(snapshot.inactive()) {
            let descriptor = &resolved.package.descriptor;
            let source_layer = layer_name(resolved.package.layer);
            let status = resolution_status_name(resolved.status);
            let active_revision_id = active_revisions.get(&descriptor.id).cloned();
            let manageable = match resolved.package.layer {
                SkillLayer::Builtin => self
                    .skill_policy
                    .can_override(&self.actor_context, &descriptor.id),
                SkillLayer::Managed => [
                    SkillOperation::Disable,
                    SkillOperation::DeleteManaged,
                    SkillOperation::Rollback,
                ]
                .into_iter()
                .any(|operation| {
                    self.skill_policy
                        .allows(&self.actor_context, operation, descriptor.kind)
                }),
                SkillLayer::Session => false,
            };
            inventory.insert(
                descriptor.id.as_str().to_string(),
                MobileSkillDto {
                    package_id: descriptor.id.as_str().to_string(),
                    display_name: descriptor.display_name.clone(),
                    version: descriptor.version.to_string(),
                    source_layer: source_layer.into(),
                    status: status.into(),
                    available: resolved.status == SkillResolutionStatus::Active,
                    reason: resolved.reason.clone(),
                    active_revision_id,
                    manageable,
                },
            );
        }
        inventory.into_values().collect()
    }

    pub fn list_managed_skills(&self) -> Result<Vec<SkillPackageStatus>> {
        self.tokio.block_on(
            self.skill_management
                .list_managed_skills(&self.actor_context),
        )
    }

    pub fn create_skill_draft(
        &self,
        request: CreateSkillDraftRequest,
    ) -> Result<SkillDraftSummary> {
        self.tokio.block_on(
            self.skill_management
                .create_draft(&self.actor_context, request),
        )
    }

    pub fn update_skill_draft(
        &self,
        revision_id: &str,
        files: Vec<DraftFileUpdate>,
    ) -> Result<SkillDraftSummary> {
        self.tokio.block_on(self.skill_management.update_draft(
            &self.actor_context,
            revision_id,
            files,
        ))
    }

    pub fn validate_skill_draft(&self, revision_id: &str) -> Result<SkillDraftValidation> {
        self.tokio.block_on(
            self.skill_management
                .validate_draft(&self.actor_context, revision_id),
        )
    }

    pub fn request_skill_activation(&self, revision_id: &str) -> Result<serde_json::Value> {
        let approval = self.tokio.block_on(
            self.skill_management
                .request_activation(&self.actor_context, revision_id),
        )?;
        Ok(approval_value(&approval))
    }

    pub fn resolve_skill_approval(
        &self,
        approval_id: &str,
        approve: bool,
    ) -> Result<serde_json::Value> {
        if approve {
            let report = self.tokio.block_on(
                self.skill_management
                    .approve_pending_skill_operation(approval_id, &self.actor_context),
            )?;
            self.record_reload(report.active_generation);
            let mut value = reload_report_value(&report);
            value["status"] = serde_json::json!("approved");
            Ok(value)
        } else {
            let approval = self.tokio.block_on(
                self.skill_management
                    .reject_pending_skill_operation(approval_id, &self.actor_context),
            )?;
            Ok(approval_value(&approval))
        }
    }

    pub fn disable_managed_skill(&self, package_id: &str) -> Result<serde_json::Value> {
        let package_id = SkillPackageId::parse(package_id)?;
        let report = self.tokio.block_on(
            self.skill_management
                .disable_managed_skill(&self.actor_context, &package_id),
        )?;
        self.record_reload(report.active_generation);
        Ok(reload_report_value(&report))
    }

    pub fn rollback_managed_skill(
        &self,
        package_id: &str,
        revision_id: &str,
    ) -> Result<serde_json::Value> {
        let package_id = SkillPackageId::parse(package_id)?;
        let outcome = self
            .tokio
            .block_on(self.skill_management.rollback_managed_skill(
                &self.actor_context,
                &package_id,
                revision_id,
            ))?;
        match outcome {
            SkillRollbackOutcome::Published(report) => {
                self.record_reload(report.generation);
                Ok(serde_json::to_value(report)?)
            }
            SkillRollbackOutcome::ApprovalRequired(approval) => Ok(approval_value(&approval)),
        }
    }

    pub fn request_skill_removal(&self, package_id: &str) -> Result<serde_json::Value> {
        let package_id = SkillPackageId::parse(package_id)?;
        let approval = self.tokio.block_on(
            self.skill_management
                .request_removal(&self.actor_context, &package_id),
        )?;
        Ok(approval_value(&approval))
    }

    fn record_reload(&self, generation: u64) {
        if let Ok(mut status) = self.last_reload_status.lock() {
            *status = format!("published_generation_{generation}");
        }
    }

    pub fn create_session(&self, title: &str) -> Result<MobileSessionDto> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("session title is required");
        }
        self.tokio
            .block_on(self.storage.create_session(title))
            .map(Into::into)
    }

    pub fn list_sessions(&self) -> Result<Vec<MobileSessionDto>> {
        self.tokio
            .block_on(self.storage.list_sessions())
            .map(|sessions| sessions.into_iter().map(Into::into).collect())
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<MobileMessageDto>> {
        self.tokio
            .block_on(self.storage.list_messages(session_id))
            .map(|messages| messages.into_iter().map(Into::into).collect())
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        self.tokio.block_on(self.storage.delete_session(session_id))
    }

    pub fn save_model_config(&self, config: MobileModelConfigDto) -> Result<()> {
        let stored = StoredModelConfig::try_from(config)?;
        self.tokio
            .block_on(self.storage.save_model_config(&stored))?;
        self.model_configured.store(true, Ordering::Release);
        Ok(())
    }

    pub fn load_model_config(&self) -> Result<Option<MobileModelConfigDto>> {
        self.tokio
            .block_on(self.storage.load_model_config())
            .map(|config| config.map(Into::into))
    }

    pub fn send_message(
        &self,
        session_id: &str,
        content: &str,
        api_key: Option<String>,
    ) -> Result<MobileTurnDto> {
        if self.cancellation.is_cancelled() {
            anyhow::bail!("runtime closed");
        }
        self.tokio.block_on(async {
            if !self.storage.session_exists(session_id).await? {
                anyhow::bail!("session not found");
            }
            self.storage
                .append_message(session_id, "user", content)
                .await?;
            Ok::<_, anyhow::Error>(())
        })?;

        let cancellation = self.cancellation.clone();
        let result = self.tokio.block_on(async {
            let turn = async {
                let config = self
                    .storage
                    .load_model_config()
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("model configuration is required"))?;
                if config.secret_id.is_some()
                    && api_key
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_none()
                {
                    anyhow::bail!("model API key is unavailable");
                }
                let host = HttpMobileRuntimeHost::new_with_manager(
                    self.storage.clone(),
                    self.skill_manager.clone(),
                    self.runtime_config.clone(),
                    self.init.clone(),
                    config,
                    TransientSecretResolver::new(api_key),
                )?;
                host.send_message_after_user_persisted(session_id, content)
                    .await
            };
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => anyhow::bail!("runtime closed"),
                result = tokio::time::timeout(Duration::from_secs(60), turn) => match result {
                    Ok(result) => result,
                    Err(_) => anyhow::bail!("model turn timed out"),
                },
            }
        })?;
        Ok(MobileTurnDto {
            assistant_text: result.assistant_text,
        })
    }

    pub fn close(&self) {
        self.cancellation.cancel();
    }
}

struct TransientSecretResolver {
    secret: Mutex<Option<String>>,
}

impl TransientSecretResolver {
    fn new(secret: Option<String>) -> Self {
        Self {
            secret: Mutex::new(secret),
        }
    }
}

#[async_trait::async_trait]
impl SecretResolver for TransientSecretResolver {
    async fn resolve_secret(&self, _secret_id: &str) -> Result<Option<String>> {
        Ok(self
            .secret
            .lock()
            .map_err(|_| anyhow::anyhow!("model secret lock is unavailable"))?
            .take())
    }
}

impl From<agent_runtime::session::Session> for MobileSessionDto {
    fn from(session: agent_runtime::session::Session) -> Self {
        Self {
            id: session.id,
            title: session.title,
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.updated_at.to_rfc3339(),
        }
    }
}

impl From<agent_runtime::session::Message> for MobileMessageDto {
    fn from(message: agent_runtime::session::Message) -> Self {
        Self {
            id: message.id,
            session_id: message.session_id,
            role: message.role,
            content: message.content,
            created_at: message.created_at.to_rfc3339(),
        }
    }
}

impl TryFrom<MobileModelConfigDto> for StoredModelConfig {
    type Error = anyhow::Error;

    fn try_from(config: MobileModelConfigDto) -> Result<Self> {
        let endpoint_type = match config.endpoint_type.as_str() {
            "responses" => EndpointType::Responses,
            "chat_completions" => EndpointType::ChatCompletions,
            "completion" => EndpointType::Completion,
            value => anyhow::bail!("unsupported endpoint type: {value}"),
        };
        let stored = Self {
            provider_id: config.provider_id,
            provider_name: config.provider_name,
            endpoint_type,
            base_url: config.base_url,
            model_name: config.model_name,
            secret_id: config.secret_id,
            headers: config.headers,
        };
        stored.validate().map_err(anyhow::Error::msg)?;
        Ok(stored)
    }
}

impl From<StoredModelConfig> for MobileModelConfigDto {
    fn from(config: StoredModelConfig) -> Self {
        Self {
            provider_id: config.provider_id,
            provider_name: config.provider_name,
            endpoint_type: match config.endpoint_type {
                EndpointType::Responses => "responses",
                EndpointType::ChatCompletions => "chat_completions",
                EndpointType::Completion => "completion",
            }
            .into(),
            base_url: config.base_url,
            model_name: config.model_name,
            secret_id: config.secret_id,
            headers: config.headers,
        }
    }
}

fn parse_platform(value: &str) -> Result<PlatformId> {
    match value {
        "android" => Ok(PlatformId::Android),
        "desktop" => Ok(PlatformId::Desktop),
        "ios" => Ok(PlatformId::Ios),
        "web" => Ok(PlatformId::Web),
        "server" => Ok(PlatformId::Server),
        _ => anyhow::bail!("unsupported platform: {value}"),
    }
}

fn platform_name(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Android => "android",
        PlatformId::Desktop => "desktop",
        PlatformId::Ios => "ios",
        PlatformId::Web => "web",
        PlatformId::Server => "server",
    }
}

fn management_mode_name(mode: SkillManagementMode) -> &'static str {
    match mode {
        SkillManagementMode::Disabled => "disabled",
        SkillManagementMode::DiagnosticsOnly => "diagnostics_only",
        SkillManagementMode::OwnerOnly => "owner_only",
        SkillManagementMode::OrganizationManaged => "organization_managed",
    }
}

fn recovery_status_name(status: RecoveryStatus) -> &'static str {
    match status {
        RecoveryStatus::CurrentSnapshotValid => "current_snapshot_valid",
        RecoveryStatus::NewSnapshotPublished => "new_snapshot_published",
        RecoveryStatus::LastKnownGoodRestored => "last_known_good_restored",
    }
}

fn layer_name(layer: SkillLayer) -> &'static str {
    match layer {
        SkillLayer::Builtin => "builtin",
        SkillLayer::Managed => "managed",
        SkillLayer::Session => "session",
    }
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
    }
}

fn reload_report_value(
    report: &agent_runtime::skill_manager::SkillReloadReport,
) -> serde_json::Value {
    serde_json::json!({
        "previous_generation": report.previous_generation,
        "active_generation": report.active_generation,
        "active_packages": report.active_packages,
        "inactive_packages": report.inactive_packages,
    })
}

fn approval_value(approval: &SkillApprovalRecord) -> serde_json::Value {
    serde_json::json!({
        "approval_id": approval.approval_id,
        "package_id": approval.package_id.as_str(),
        "permission_diff": approval.permission_diff,
        "requested_by": approval.requested_by,
        "revision_id": approval.revision_id,
        "status": approval.status.as_str(),
    })
}

fn count_real_directories(root: &Path) -> usize {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_type().ok())
        .filter(|file_type| file_type.is_dir() && !file_type.is_symlink())
        .count()
}

fn ensure_distinct_roots(roots: &[&Path]) -> Result<()> {
    for (index, left) in roots.iter().enumerate() {
        for right in roots.iter().skip(index + 1) {
            if left.starts_with(right) || right.starts_with(left) {
                anyhow::bail!("skill layer roots must be separate app-private directories");
            }
        }
    }
    Ok(())
}

fn ensure_configured_store_root(configured: &Path, prepared: &Path, label: &str) -> Result<()> {
    let configured = configured
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {label}"))?;
    let prepared = prepared
        .canonicalize()
        .with_context(|| format!("failed to canonicalize prepared {label}"))?;
    if configured != prepared {
        anyhow::bail!("{label} must use the app-private managed skill layout");
    }
    Ok(())
}

fn prepare_private_root(path: &str) -> Result<PathBuf> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        anyhow::bail!("app-private root must be a real directory: {path}");
    }
    std::fs::create_dir_all(path)?;
    Path::new(path)
        .canonicalize()
        .with_context(|| format!("failed to canonicalize app-private root: {path}"))
}

fn resolve_private_path(
    raw_path: &str,
    default_root: &Path,
    allowed_roots: &[PathBuf],
    label: &str,
) -> Result<PathBuf> {
    let candidate = Path::new(raw_path);
    let absolute_candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        default_root.join(candidate)
    };
    let resolved_path = canonicalize_existing_ancestors(&absolute_candidate)?;

    if allowed_roots
        .iter()
        .any(|root| resolved_path.starts_with(root))
    {
        Ok(resolved_path)
    } else {
        anyhow::bail!("{label} must stay inside app-private storage")
    }
}

fn canonicalize_existing_ancestors(path: &Path) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            std::path::Component::RootDir => resolved.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                anyhow::bail!("path must stay inside app-private storage")
            }
            std::path::Component::Normal(part) => {
                let next = resolved.join(part);
                if next.exists() {
                    resolved = next.canonicalize().with_context(|| {
                        format!("failed to canonicalize existing path: {}", next.display())
                    })?;
                } else {
                    resolved = next;
                }
            }
        }
    }

    Ok(resolved)
}
