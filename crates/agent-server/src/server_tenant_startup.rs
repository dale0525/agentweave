use crate::api;
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_policy::SkillManagementPolicy;
use agent_runtime::tools::RuntimeConfig;
use agent_runtime::turn::ModelClient;
use agent_server::owner_api::OwnerApiConfig;
use agent_server::tenant_skills::{
    FilesystemTenantSkillManagerFactory, TenantSkillManagerConfig, TenantSkillManagerRegistry,
    TenantSkillRuntime,
};
use std::path::Path;
use std::sync::Arc;

use super::server_skill_startup::{
    BuiltinSkillsMode, ManagedSkillsConfig, load_builtin_skill_source,
};

pub(super) async fn build_managed_tenant_registry(
    skills_root: &Path,
    managed: ManagedSkillsConfig,
    builtin_mode: BuiltinSkillsMode,
    management_policy: SkillManagementPolicy,
) -> anyhow::Result<TenantSkillManagerRegistry> {
    let builtin = load_builtin_skill_source(skills_root, builtin_mode).await?;
    let factory = FilesystemTenantSkillManagerFactory::new(TenantSkillManagerConfig {
        data_root: managed.app_data_root,
        cache_root: managed.cache_root,
        sources: vec![builtin],
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::desktop_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: env!("CARGO_PKG_VERSION").parse()?,
        management_policy,
    })
    .await?;
    Ok(TenantSkillManagerRegistry::new(factory))
}

pub(super) fn build_tenant_app_state<C>(
    runtime: Arc<TenantSkillRuntime>,
    model: C,
    runtime_config: RuntimeConfig,
    owner_management: Option<OwnerApiConfig>,
) -> api::AppState
where
    C: ModelClient + 'static,
{
    if let Some(owner_management) = owner_management {
        api::AppState::new_with_model_skill_manager_and_owner(
            runtime.storage.clone(),
            model,
            runtime.manager.clone(),
            runtime_config,
            owner_management,
        )
    } else {
        api::AppState::new_with_model_and_skill_manager(
            runtime.storage.clone(),
            model,
            runtime.manager.clone(),
            runtime_config,
        )
    }
}
