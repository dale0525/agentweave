use agent_runtime::app_definition::{
    AgentAppHostDiscovery, AgentAppRuntimeInventory, AgentAppRuntimePolicy, ResolvedAgentApp,
};
use agent_runtime::app_manifest::AppNetworkPolicy;
use agent_runtime::attachment_tools::AttachmentToolRuntime;
use agent_runtime::attachments::{AttachmentScope, SqliteAttachmentStore};
use agent_runtime::automation_tools::{AutomationScope, AutomationToolRuntime};
use agent_runtime::connector::ConnectorRuntime;
use agent_runtime::connector_tools::{ConnectorToolRuntime, EphemeralConnectorContextProvider};
use agent_runtime::credential::{ConnectorAccount, CredentialScope, ProviderCredential};
use agent_runtime::mail::{MailAccount, MailAddress, MailConnector};
use agent_runtime::mail_attachments::{MailAttachmentSource, StoredMailAttachmentSource};
use agent_runtime::mail_connector_transport::{
    MAIL_CONNECTOR_ID, MAIL_TOOL_NAMES, MailConnectorTransport,
};
use agent_runtime::mail_fake::FakeMailConnector;
use agent_runtime::mail_imap_smtp::{ImapSmtpMailConfig, ImapSmtpMailConnector};
use agent_runtime::memory::{MemoryProvider, MemoryScope};
use agent_runtime::memory_tools::MemoryToolRuntime;
use agent_runtime::platform::PlatformId;
use agent_runtime::prompt_composer::AppPromptConfig;
use agent_runtime::skill_manager::SkillManager;
use agent_runtime::storage::Storage;
use agent_runtime::task_tools::TaskToolRuntime;
use agent_runtime::tasks::{TaskProvider, TaskScope};
use agent_runtime::tools::RuntimeConfig;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(super) struct ResolvedConnectorFoundation {
    pub(super) tools: ConnectorToolRuntime,
    pub(super) actions: agent_runtime::foundation_actions::MailActionService,
}

pub(super) struct ResolvedServerApp {
    pub(super) prompt: AppPromptConfig,
    pub(super) host_discovery: Option<AgentAppHostDiscovery>,
    pub(super) runtime_policy: Option<AgentAppRuntimePolicy>,
}

impl ResolvedServerApp {
    pub(super) fn enforce_runtime_policy(&self, runtime_config: RuntimeConfig) -> RuntimeConfig {
        match &self.runtime_policy {
            Some(policy) => runtime_config.with_agent_app_policy(policy.clone()),
            None => runtime_config,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailConnectorMode {
    Unconfigured,
    Fake,
    ImapSmtp,
}

pub(super) async fn resolve_app(
    manager: &SkillManager,
    runtime_config: &RuntimeConfig,
) -> anyhow::Result<ResolvedServerApp> {
    let Ok(root) = std::env::var("AGENTWEAVE_APP_ROOT") else {
        return Ok(ResolvedServerApp {
            prompt: AppPromptConfig::default(),
            host_discovery: None,
            runtime_policy: None,
        });
    };
    let snapshot = manager.current_snapshot();
    let capabilities = manager
        .runtime_context()
        .map(|context| {
            context
                .capabilities()
                .names()
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .chain(first_party_capabilities())
        .collect();
    let inventory = AgentAppRuntimeInventory {
        runtime_version: env!("CARGO_PKG_VERSION").parse()?,
        platform: manager
            .runtime_context()
            .map_or(PlatformId::Desktop, |context| context.platform()),
        packages: snapshot
            .packages()
            .iter()
            .map(|resolved| {
                (
                    resolved.package.descriptor.id.as_str().to_string(),
                    resolved.package.descriptor.version.clone(),
                )
            })
            .collect(),
        capabilities,
        runtime_tools: snapshot
            .registry()
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .chain(crate::server_app::first_party_tool_names())
            .collect(),
        connectors: runtime_config
            .connectors
            .iter()
            .map(|connector| connector.id.clone())
            .chain([MAIL_CONNECTOR_ID.to_string()])
            .collect(),
    };
    let resolved =
        ResolvedAgentApp::load(PathBuf::from(root).as_path(), &inventory, 64 * 1024).await?;
    let host_discovery = resolved.host_discovery().clone();
    let runtime_policy = resolved.runtime_policy().clone();
    Ok(ResolvedServerApp {
        prompt: resolved.prompt,
        host_discovery: Some(host_discovery),
        runtime_policy: Some(runtime_policy),
    })
}

fn first_party_capabilities() -> impl Iterator<Item = String> {
    [
        "memory-provider",
        "provenance",
        "retention-policy",
        "reversible-history",
        "durable-actions",
        "approval-engine",
        "credential-vault",
        "mail-connector",
        "host-tools",
        "task-provider",
        "scheduler",
        "attachments",
        "data-protection",
    ]
    .into_iter()
    .map(str::to_string)
}

fn first_party_tool_names() -> impl Iterator<Item = String> {
    agent_runtime::memory_tools::MEMORY_TOOL_NAMES
        .into_iter()
        .chain(agent_runtime::task_tools::TASK_TOOL_NAMES)
        .chain(agent_runtime::automation_tools::AUTOMATION_TOOL_NAMES)
        .chain(agent_runtime::attachment_tools::ATTACHMENT_TOOL_NAMES)
        .chain(MAIL_TOOL_NAMES)
        .map(str::to_string)
}

pub(super) async fn resolve_memory_tools(
    storage: &Storage,
    app_prompt: &AppPromptConfig,
) -> anyhow::Result<Option<MemoryToolRuntime>> {
    let enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "memory-provider")
        || std::env::var("AGENTWEAVE_MEMORY").as_deref() == Ok("enabled");
    if !enabled {
        return Ok(None);
    }
    let provider = Arc::new(storage.local_memory_provider());
    provider.initialize().await?;
    Ok(Some(MemoryToolRuntime::new(
        provider,
        MemoryScope::new(&app_prompt.identity.app_id, "local", "local-user")?,
    )?))
}

pub(super) async fn resolve_task_tools(
    storage: &Storage,
    app_prompt: &AppPromptConfig,
) -> anyhow::Result<Option<TaskToolRuntime>> {
    let enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "task-provider")
        || std::env::var("AGENTWEAVE_TASKS").as_deref() == Ok("enabled");
    if !enabled {
        return Ok(None);
    }
    let provider = Arc::new(storage.local_task_provider());
    provider.initialize().await?;
    Ok(Some(TaskToolRuntime::new(
        provider,
        TaskScope::new(&app_prompt.identity.app_id, "local", "local-user")?,
    )?))
}

pub(super) async fn resolve_automation_tools(
    storage: &Storage,
    app_prompt: &AppPromptConfig,
    runtime_config: &RuntimeConfig,
) -> anyhow::Result<Option<AutomationToolRuntime>> {
    let declared = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "scheduler");
    let enabled_by_host = std::env::var("AGENTWEAVE_AUTOMATION").as_deref() == Ok("enabled");
    if !background_execution_allowed(runtime_config, declared, enabled_by_host) {
        return Ok(None);
    }
    AutomationToolRuntime::from_storage(
        storage,
        AutomationScope::new(&app_prompt.identity.app_id, "local", "local-user")?,
    )
    .await
    .map(Some)
}

pub(super) async fn resolve_attachment_tools(
    storage: &Storage,
    app_prompt: &AppPromptConfig,
) -> anyhow::Result<Option<AttachmentToolRuntime>> {
    let enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "attachments")
        || std::env::var("AGENTWEAVE_ATTACHMENTS").as_deref() == Ok("enabled");
    if !enabled {
        return Ok(None);
    }
    Ok(Some(AttachmentToolRuntime::new(
        SqliteAttachmentStore::from_storage(storage).await?,
        AttachmentScope::new(&app_prompt.identity.app_id, "local", "local-user")?,
    )))
}

pub(super) async fn resolve_connector_tools(
    storage: &Storage,
    app_prompt: &AppPromptConfig,
    runtime_config: &RuntimeConfig,
) -> anyhow::Result<Option<ResolvedConnectorFoundation>> {
    let declared = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "mail-connector");
    let enabled_by_host = std::env::var("AGENTWEAVE_FAKE_MAIL").as_deref() == Ok("enabled");
    let enabled = mail_foundation_allowed(runtime_config, declared, enabled_by_host);
    if !enabled {
        return Ok(None);
    }

    let ledger = Arc::new(
        agent_runtime::connector_ledger::SqliteConnectorActionLedger::from_storage(storage).await?,
    );
    let attachments_enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "attachments")
        || std::env::var("AGENTWEAVE_ATTACHMENTS").as_deref() == Ok("enabled");
    let attachment_source: Option<Arc<dyn MailAttachmentSource>> = if attachments_enabled {
        Some(Arc::new(StoredMailAttachmentSource::new(
            SqliteAttachmentStore::from_storage(storage).await?,
            AttachmentScope::new(&app_prompt.identity.app_id, "local", "local-user")?,
        )) as Arc<dyn MailAttachmentSource>)
    } else {
        None
    };
    let vault = resolve_credential_vault(storage).await?;
    let (mail, display_name, deterministic): (Arc<dyn MailConnector>, &str, bool) =
        match mail_connector_mode_from_lookup(|name| std::env::var_os(name))? {
            MailConnectorMode::ImapSmtp => {
                let config = load_imap_smtp_config().await?;
                anyhow::ensure!(
                    config.credential_scope.app_id == app_prompt.identity.app_id,
                    "IMAP/SMTP credential scope App does not match the active Agent App"
                );
                let configured_vault = vault.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("IMAP/SMTP requires the persistent Credential Vault")
                })?;
                let granted_scopes = BTreeSet::from([
                    "mail.message.read".into(),
                    "mail.message.organize".into(),
                    "mail.message.send".into(),
                ]);
                let credential_id = config.credential_secret_id.as_str().to_string();
                configured_vault
                    .register_provider_credential_persistent(
                        &config.credential_scope,
                        ProviderCredential {
                            access_secret_id: config.credential_secret_id.clone(),
                            credential_id: credential_id.clone(),
                            expires_at: None,
                            granted_scopes: granted_scopes.clone(),
                            provider_id: "imap-smtp".into(),
                            provider_subject: config.username.clone(),
                            refresh_secret_id: None,
                            revoked_at: None,
                        },
                    )
                    .await?;
                configured_vault
                    .register_account_persistent(ConnectorAccount {
                        account_id: config.account.id.clone(),
                        allowed_scopes: granted_scopes,
                        connector_id: "agentweave.connector.mail.imap-smtp".into(),
                        credential_id,
                        scope: config.credential_scope.clone(),
                    })
                    .await?;
                let mut connector =
                    ImapSmtpMailConnector::new(config, Arc::new(configured_vault.clone()))?;
                if let Some(source) = &attachment_source {
                    connector = connector.with_attachment_source(source.clone());
                }
                (Arc::new(connector), "IMAP/SMTP Mail", false)
            }
            MailConnectorMode::Fake => {
                let fake = Arc::new(FakeMailConnector::new());
                fake.add_account(MailAccount {
                    id: "primary".into(),
                    display_name: "Example Mail".into(),
                    primary_address: MailAddress {
                        name: Some("Local User".into()),
                        address: "local@example.test".into(),
                    },
                    addresses: Vec::new(),
                    provider_reference: None,
                })?;
                (fake, "Fake Mail", true)
            }
            MailConnectorMode::Unconfigured => (
                Arc::new(FakeMailConnector::new()),
                "Unconfigured Mail",
                true,
            ),
        };
    let runtime = Arc::new(ConnectorRuntime::new_with_ledger(
        vault,
        ledger,
        256 * 1024,
    )?);
    runtime
        .register(
            MailConnectorTransport::descriptor(display_name, deterministic),
            Arc::new(MailConnectorTransport::new(mail)),
        )
        .await?;
    let scope = CredentialScope {
        app_id: app_prompt.identity.app_id.clone(),
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let context = Arc::new(EphemeralConnectorContextProvider::fail_closed(
        scope.clone(),
        Duration::from_secs(30),
    )?);
    let tools = ConnectorToolRuntime::load(runtime, context.clone())?;
    let actions = agent_runtime::foundation_actions::MailActionService::new(
        storage,
        tools.clone(),
        context,
        scope,
        "agentweave.foundation-actions.v1",
    )
    .await?;
    Ok(Some(ResolvedConnectorFoundation { tools, actions }))
}

pub(super) fn background_execution_allowed(
    runtime_config: &RuntimeConfig,
    declared_by_app: bool,
    enabled_by_host: bool,
) -> bool {
    runtime_config
        .agent_app_policy
        .as_ref()
        .map_or(declared_by_app || enabled_by_host, |policy| {
            policy.allows_background_execution(declared_by_app, enabled_by_host)
        })
}

fn mail_foundation_allowed(
    runtime_config: &RuntimeConfig,
    declared_by_app: bool,
    enabled_by_host: bool,
) -> bool {
    runtime_config
        .agent_app_policy
        .as_ref()
        .map_or(declared_by_app || enabled_by_host, |policy| {
            policy.network() != AppNetworkPolicy::Deny
                && policy.declares_connector(MAIL_CONNECTOR_ID)
                && declared_by_app
        })
}

fn mail_connector_mode_from_lookup<F>(lookup: F) -> anyhow::Result<MailConnectorMode>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    let connector = lookup("AGENTWEAVE_MAIL_CONNECTOR")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("AGENTWEAVE_MAIL_CONNECTOR must be valid UTF-8"))
        })
        .transpose()?;
    match connector.as_deref() {
        Some("imap-smtp") => Ok(MailConnectorMode::ImapSmtp),
        Some(value) => anyhow::bail!("unsupported AGENTWEAVE_MAIL_CONNECTOR '{value}'"),
        None if lookup("AGENTWEAVE_FAKE_MAIL").as_deref()
            == Some(std::ffi::OsStr::new("enabled")) =>
        {
            Ok(MailConnectorMode::Fake)
        }
        None => Ok(MailConnectorMode::Unconfigured),
    }
}

async fn load_imap_smtp_config() -> anyhow::Result<ImapSmtpMailConfig> {
    let path = PathBuf::from(
        std::env::var("AGENTWEAVE_MAIL_ACCOUNT_CONFIG").map_err(|_| {
            anyhow::anyhow!("AGENTWEAVE_MAIL_ACCOUNT_CONFIG is required for IMAP/SMTP")
        })?,
    );
    let metadata = tokio::fs::symlink_metadata(&path).await?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "IMAP/SMTP account config must be a real file"
    );
    anyhow::ensure!(
        metadata.len() <= 64 * 1024,
        "IMAP/SMTP account config is too large"
    );
    let config: ImapSmtpMailConfig = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    config.validate()?;
    Ok(config)
}

async fn resolve_credential_vault(
    storage: &Storage,
) -> anyhow::Result<Option<agent_runtime::credential::CredentialVault>> {
    let configured = match (
        std::env::var("AGENTWEAVE_SECRET_ROOT").ok(),
        std::env::var("AGENTWEAVE_SECRET_MASTER_KEY_HEX").ok(),
    ) {
        (None, None) => return Ok(None),
        (Some(root), Some(key)) => (root, key),
        _ => anyhow::bail!(
            "AGENTWEAVE_SECRET_ROOT and AGENTWEAVE_SECRET_MASTER_KEY_HEX must be configured together"
        ),
    };
    let key = hex::decode(configured.1)?;
    anyhow::ensure!(key.len() == 32, "secret master key must decode to 32 bytes");
    let store = Arc::new(
        agent_runtime::credential_file::EncryptedFileSecretStore::new(
            configured.0,
            agent_runtime::credential::SecretMaterial::new(key)?,
        )?,
    );
    let metadata =
        agent_runtime::credential_sqlite::SqliteCredentialMetadataStore::from_storage(storage)
            .await?;
    Ok(Some(
        agent_runtime::credential::CredentialVault::new_persistent(store, metadata),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::app_definition::AgentAppRuntimePolicy;
    use agent_runtime::app_manifest::AgentAppManifest;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn connector_mode(values: &[(&str, &str)]) -> anyhow::Result<MailConnectorMode> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        mail_connector_mode_from_lookup(|name| values.get(name).cloned())
    }

    fn runtime_config_with_policy(
        network: &str,
        background: &str,
        connectors: &[&str],
    ) -> RuntimeConfig {
        let manifest = serde_json::json!({
            "schemaVersion": 1,
            "appId": "com.example.policy-test",
            "package": {"id": "com.example.policy-test.app", "version": "0.1.0"},
            "requires": {
                "packages": [],
                "capabilities": [],
                "runtimeTools": [],
                "connectors": connectors
            },
            "features": [],
            "policy": {
                "externalSideEffects": "require_approval",
                "network": network,
                "backgroundExecution": background,
                "memoryPersistence": "disabled",
                "skillManagement": "disabled"
            },
            "branding": {"displayName": "Policy Test"},
            "instructions": {"system": "prompts/system.md"}
        });
        let manifest =
            AgentAppManifest::parse_json(&serde_json::to_vec(&manifest).unwrap()).unwrap();
        RuntimeConfig::workspace_write(".", ".")
            .with_agent_app_policy(AgentAppRuntimePolicy::compile(&manifest))
    }

    #[test]
    fn mail_connector_defaults_to_an_unconfigured_account_set() {
        assert_eq!(
            connector_mode(&[]).unwrap(),
            MailConnectorMode::Unconfigured
        );
    }

    #[test]
    fn fake_mail_requires_an_explicit_test_flag() {
        assert_eq!(
            connector_mode(&[("AGENTWEAVE_FAKE_MAIL", "enabled")]).unwrap(),
            MailConnectorMode::Fake
        );
    }

    #[test]
    fn imap_smtp_is_selected_explicitly_and_unknown_connectors_fail_closed() {
        assert_eq!(
            connector_mode(&[("AGENTWEAVE_MAIL_CONNECTOR", "imap-smtp")]).unwrap(),
            MailConnectorMode::ImapSmtp
        );
        assert!(connector_mode(&[("AGENTWEAVE_MAIL_CONNECTOR", "unknown")]).is_err());
    }

    #[test]
    fn manifest_background_policy_cannot_be_bypassed_by_host_flags() {
        let disabled = runtime_config_with_policy("deny", "disabled", &[]);
        assert!(!background_execution_allowed(&disabled, true, true));

        let declared = runtime_config_with_policy("deny", "declared_only", &[]);
        assert!(!background_execution_allowed(&declared, false, true));
        assert!(background_execution_allowed(&declared, true, false));

        let enabled = runtime_config_with_policy("deny", "enabled", &[]);
        assert!(background_execution_allowed(&enabled, false, true));
    }

    #[test]
    fn manifest_network_policy_cannot_be_bypassed_by_fake_mail_flag() {
        let denied = runtime_config_with_policy("deny", "disabled", &[MAIL_CONNECTOR_ID]);
        assert!(!mail_foundation_allowed(&denied, true, true));

        let undeclared = runtime_config_with_policy("declared_only", "disabled", &[]);
        assert!(!mail_foundation_allowed(&undeclared, true, true));

        let declared =
            runtime_config_with_policy("declared_only", "disabled", &[MAIL_CONNECTOR_ID]);
        assert!(mail_foundation_allowed(&declared, true, false));
        assert!(!mail_foundation_allowed(&declared, false, true));
    }
}
