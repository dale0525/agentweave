use crate::events::RuntimeEvent;
use crate::model_config::StoredModelConfig;
use crate::platform::{CapabilitySet, PlatformId};
use crate::session::{Message, Session};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_manager::SkillManager;
use crate::skill_policy::ActorContext;
use crate::storage::Storage;
use crate::tools::RuntimeConfig;
use crate::turn::{ModelClient, TurnRunner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileRuntimeInit {
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MobileTurnResult {
    pub assistant_text: String,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileRuntimeDiagnostics {
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
    pub built_in_tools_enabled: bool,
    pub registered_skill_tool_count: usize,
    pub configured_external_tool_count: usize,
    pub configured_connector_count: usize,
}

#[async_trait::async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve_secret(&self, secret_id: &str) -> anyhow::Result<Option<String>>;
}

pub async fn resolve_model_api_key<R>(
    model_config: &StoredModelConfig,
    resolver: &R,
) -> anyhow::Result<Option<String>>
where
    R: SecretResolver,
{
    match &model_config.secret_id {
        Some(secret_id) => resolver.resolve_secret(secret_id).await,
        None => Ok(None),
    }
}

pub struct MobileRuntimeHost<C> {
    storage: Storage,
    model: Arc<C>,
    skill_manager: SkillManager,
    runtime_config: RuntimeConfig,
    init: MobileRuntimeInit,
}

impl<C> MobileRuntimeHost<C>
where
    C: ModelClient,
{
    pub fn new_for_test(
        storage: Storage,
        model: C,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
    ) -> Self {
        let skill_manager = SkillManager::from_registry_and_catalog_with_context(
            skills,
            skill_catalog,
            init.platform,
            init.capabilities.clone(),
        );
        Self::new_for_test_with_manager(storage, model, skill_manager, runtime_config, init)
            .expect("mobile manager created from init must have matching runtime context")
    }

    pub fn new_for_test_with_manager(
        storage: Storage,
        model: C,
        skill_manager: SkillManager,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
    ) -> anyhow::Result<Self> {
        validate_mobile_manager_context(&init, &skill_manager)?;
        let runtime_config = mobile_safe_runtime_config(&init, runtime_config);
        Ok(Self {
            storage,
            model: Arc::new(model),
            skill_manager,
            runtime_config,
            init,
        })
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        self.storage.create_session(title).await
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.storage.list_sessions().await
    }

    pub async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        self.storage.list_messages(session_id).await
    }

    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.storage.delete_session(session_id).await
    }

    pub fn init(&self) -> &MobileRuntimeInit {
        &self.init
    }

    pub fn diagnostics(&self) -> MobileRuntimeDiagnostics {
        let skill_manager = mobile_safe_snapshot_manager(&self.init, &self.skill_manager);
        let snapshot = skill_manager.current_snapshot();
        mobile_runtime_diagnostics(&self.init, &self.runtime_config, snapshot.registry())
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        if !self.storage.session_exists(session_id).await? {
            anyhow::bail!("session not found");
        }
        self.storage
            .append_message(session_id, "user", content)
            .await?;
        let skill_manager = mobile_safe_snapshot_manager(&self.init, &self.skill_manager);
        let runner = TurnRunner::new_with_manager_and_config(
            self.model.clone(),
            skill_manager,
            self.runtime_config.clone(),
        );
        let events = runner.run(content).await?;
        let assistant_text = assistant_text_from_events(&events);
        self.storage
            .append_message(session_id, "assistant", &assistant_text)
            .await?;
        Ok(MobileTurnResult {
            assistant_text,
            events,
        })
    }
}

pub struct HttpMobileRuntimeHost<R> {
    storage: Storage,
    skill_manager: SkillManager,
    runtime_config: RuntimeConfig,
    init: MobileRuntimeInit,
    model_config: StoredModelConfig,
    secret_resolver: R,
    management: Option<(OwnerSkillManagementService, ActorContext)>,
}

impl<R> HttpMobileRuntimeHost<R>
where
    R: SecretResolver,
{
    #[deprecated(note = "production mobile hosts must use new_with_manager")]
    pub fn new(
        storage: Storage,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
        model_config: StoredModelConfig,
        secret_resolver: R,
    ) -> Self {
        let skill_manager = SkillManager::from_registry_and_catalog_with_context(
            skills,
            skill_catalog,
            init.platform,
            init.capabilities.clone(),
        );
        Self::new_with_manager(
            storage,
            skill_manager,
            runtime_config,
            init,
            model_config,
            secret_resolver,
        )
        .expect("mobile manager created from init must have matching runtime context")
    }

    pub fn new_with_manager(
        storage: Storage,
        skill_manager: SkillManager,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
        model_config: StoredModelConfig,
        secret_resolver: R,
    ) -> anyhow::Result<Self> {
        validate_mobile_manager_context(&init, &skill_manager)?;
        let runtime_config = mobile_safe_runtime_config(&init, runtime_config);
        Ok(Self {
            storage,
            skill_manager,
            runtime_config,
            init,
            model_config,
            secret_resolver,
            management: None,
        })
    }

    pub fn with_owner_turn_context(
        mut self,
        service: OwnerSkillManagementService,
        actor: ActorContext,
    ) -> Self {
        if service.policy().can_author_conversationally(&actor) {
            self.management = Some((service, actor));
        }
        self
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        self.storage.create_session(title).await
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.storage.list_sessions().await
    }

    pub async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        self.storage.list_messages(session_id).await
    }

    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.storage.delete_session(session_id).await
    }

    pub fn init(&self) -> &MobileRuntimeInit {
        &self.init
    }

    pub fn diagnostics(&self) -> MobileRuntimeDiagnostics {
        let skill_manager = mobile_safe_snapshot_manager(&self.init, &self.skill_manager);
        let snapshot = skill_manager.current_snapshot();
        mobile_runtime_diagnostics(&self.init, &self.runtime_config, snapshot.registry())
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        if !self.storage.session_exists(session_id).await? {
            anyhow::bail!("session not found");
        }
        self.storage
            .append_message(session_id, "user", content)
            .await?;
        self.send_message_after_user_persisted(session_id, content)
            .await
    }

    pub async fn send_message_after_user_persisted(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        self.model_config
            .validate()
            .map_err(|message| anyhow::anyhow!(message))?;
        let api_key = resolve_model_api_key(&self.model_config, &self.secret_resolver).await?;
        let profile = self.model_config.to_provider_profile(api_key);
        let skill_manager = mobile_safe_snapshot_manager(&self.init, &self.skill_manager);
        let mut runner = TurnRunner::new_with_manager_and_config(
            model_gateway::responses::GatewayHttpClient::new(profile),
            skill_manager,
            self.runtime_config.clone(),
        );
        let events = if let Some((service, actor)) = &self.management {
            runner = runner.with_skill_management(service.clone());
            runner
                .run_request(
                    crate::turn_request::TurnRequest::new(content)
                        .with_actor_context(actor.clone()),
                )
                .await?
        } else {
            runner.run(content).await?
        };
        let assistant_text = assistant_text_from_events(&events);
        self.storage
            .append_message(session_id, "assistant", &assistant_text)
            .await?;
        Ok(MobileTurnResult {
            assistant_text,
            events,
        })
    }
}

fn mobile_safe_runtime_config(
    init: &MobileRuntimeInit,
    mut runtime_config: RuntimeConfig,
) -> RuntimeConfig {
    if init.platform == PlatformId::Android {
        runtime_config = runtime_config.without_builtin_tools();
        runtime_config.external_tools.clear();
        runtime_config.connectors.clear();
        runtime_config
    } else {
        runtime_config
    }
}

fn mobile_safe_skill_registry(init: &MobileRuntimeInit, skills: SkillRegistry) -> SkillRegistry {
    if init.platform == PlatformId::Android {
        skills.with_platform_capabilities(init.platform, init.capabilities.clone())
    } else {
        skills
    }
}

fn mobile_safe_snapshot_manager(
    init: &MobileRuntimeInit,
    skill_manager: &SkillManager,
) -> SkillManager {
    let snapshot = skill_manager.current_snapshot();
    let registry = mobile_safe_skill_registry(init, snapshot.registry().clone());
    SkillManager::from_registry_and_catalog_with_context(
        registry,
        snapshot.catalog().clone(),
        init.platform,
        init.capabilities.clone(),
    )
}

fn validate_mobile_manager_context(
    init: &MobileRuntimeInit,
    skill_manager: &SkillManager,
) -> anyhow::Result<()> {
    let context = skill_manager
        .runtime_context()
        .ok_or_else(|| anyhow::anyhow!("mobile skill manager runtime context is required"))?;
    if context.platform() != init.platform || context.capabilities() != &init.capabilities {
        anyhow::bail!("mobile skill manager runtime context does not match mobile init");
    }
    Ok(())
}

fn mobile_runtime_diagnostics(
    init: &MobileRuntimeInit,
    runtime_config: &RuntimeConfig,
    skills: &SkillRegistry,
) -> MobileRuntimeDiagnostics {
    MobileRuntimeDiagnostics {
        platform: init.platform,
        capabilities: init.capabilities.clone(),
        built_in_tools_enabled: runtime_config.built_in_tools_enabled,
        registered_skill_tool_count: skills.tools().len(),
        configured_external_tool_count: runtime_config.external_tools.len(),
        configured_connector_count: runtime_config.connectors.len(),
    }
}

pub(crate) fn assistant_text_from_events(events: &[RuntimeEvent]) -> String {
    events
        .iter()
        .find_map(|event| {
            if let RuntimeEvent::AssistantMessageFinished { text } = event {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            events
                .iter()
                .filter_map(|event| {
                    if let RuntimeEvent::AssistantTextDelta { text } = event {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
}

#[async_trait::async_trait]
impl<C> ModelClient for Arc<C>
where
    C: ModelClient,
{
    async fn stream(
        &self,
        request: model_gateway::responses::GatewayRequest,
    ) -> anyhow::Result<crate::turn::ModelEventStream> {
        self.as_ref().stream(request).await
    }
}

#[cfg(test)]
#[path = "mobile_host_tests.rs"]
mod tests;
