use agent_runtime::app_definition::{
    AgentAppHostDiscovery, AgentAppRuntimeInventory, ResolvedAgentApp,
};
use agent_runtime::attachment_tools::AttachmentToolRuntime;
use agent_runtime::attachments::{AttachmentScope, SqliteAttachmentStore};
use agent_runtime::automation_tools::{AutomationScope, AutomationToolRuntime};
use agent_runtime::connector::ConnectorRuntime;
use agent_runtime::connector_tools::{ConnectorToolRuntime, EphemeralConnectorContextProvider};
use agent_runtime::credential::{
    ConnectorAccount, CredentialScope, CredentialVault, SecretMaterial,
};
use agent_runtime::mail::{MailAccount, MailAddress, MailConnector};
use agent_runtime::mail_connector_transport::{
    MAIL_CONNECTOR_ID, MAIL_TOOL_NAMES, MailConnectorTransport,
};
use agent_runtime::mail_fake::FakeMailConnector;
use agent_runtime::mail_imap_smtp::{ImapSmtpMailConfig, ImapSmtpMailConnector};
use agent_runtime::mail_imap_smtp_accounts::{
    ImapSmtpMailAccountManager, ManagedImapSmtpMailConnector, SqliteImapSmtpMailAccountStore,
};
use agent_runtime::memory::{MemoryProvider, MemoryScope};
use agent_runtime::memory_tools::MemoryToolRuntime;
use agent_runtime::platform::PlatformId;
use agent_runtime::prompt_composer::AppPromptConfig;
use agent_runtime::skill_manager::SkillManager;
use agent_runtime::storage::Storage;
use agent_runtime::task_tools::TaskToolRuntime;
use agent_runtime::tasks::{TaskProvider, TaskScope};
use agent_runtime::tools::RuntimeConfig;
use agent_server::api;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct ResolvedConnectorFoundation {
    pub(super) tools: ConnectorToolRuntime,
    pub(super) actions: agent_runtime::foundation_actions::MailActionService,
    pub(super) account_manager: Option<Arc<ImapSmtpMailAccountManager>>,
}

pub(super) struct ResolvedServerApp {
    pub(super) prompt: AppPromptConfig,
    pub(super) host_discovery: Option<AgentAppHostDiscovery>,
}

pub(super) fn apply_connector_foundation(
    state: api::AppState,
    foundation: Option<ResolvedConnectorFoundation>,
) -> api::AppState {
    let Some(foundation) = foundation else {
        return state;
    };
    let state = state.with_mail_actions(foundation.actions);
    match foundation.account_manager {
        Some(manager) => state.with_mail_account_manager(manager),
        None => state,
    }
}

pub(super) fn credential_root_for_database(database_path: Option<&Path>) -> Option<PathBuf> {
    database_path
        .and_then(Path::parent)
        .map(|parent| parent.join("credentials"))
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
    Ok(ResolvedServerApp {
        prompt: resolved.prompt,
        host_discovery: Some(host_discovery),
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
) -> anyhow::Result<Option<AutomationToolRuntime>> {
    let enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "scheduler")
        || std::env::var("AGENTWEAVE_AUTOMATION").as_deref() == Ok("enabled");
    if !enabled {
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
    credential_vault_key: Option<Arc<SecretMaterial>>,
    credential_root: Option<&Path>,
) -> anyhow::Result<Option<ResolvedConnectorFoundation>> {
    let enabled = app_prompt
        .identity
        .enabled_capabilities
        .iter()
        .any(|capability| capability == "mail-connector")
        || std::env::var("AGENTWEAVE_FAKE_MAIL").as_deref() == Ok("enabled");
    if !enabled {
        return Ok(None);
    }

    let ledger = Arc::new(
        agent_runtime::connector_ledger::SqliteConnectorActionLedger::from_storage(storage).await?,
    );
    let scope = CredentialScope {
        app_id: app_prompt.identity.app_id.clone(),
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let vault =
        resolve_credential_vault(storage, credential_vault_key.as_deref(), credential_root).await?;
    let (mail, display_name, deterministic, account_manager): (
        Arc<dyn MailConnector>,
        &str,
        bool,
        Option<Arc<ImapSmtpMailAccountManager>>,
    ) = match mail_connector_mode_from_lookup(|name| std::env::var_os(name))? {
        MailConnectorMode::ImapSmtp => {
            let config = load_imap_smtp_config().await?;
            anyhow::ensure!(
                config.credential_scope.app_id == app_prompt.identity.app_id,
                "IMAP/SMTP credential scope App does not match the active Agent App"
            );
            let configured_vault = vault.as_ref().ok_or_else(|| {
                anyhow::anyhow!("IMAP/SMTP requires the persistent Credential Vault")
            })?;
            configured_vault
                .register_account_persistent(ConnectorAccount {
                    account_id: config.account.id.clone(),
                    connector_id: "agentweave.connector.mail.imap-smtp".into(),
                    provider_id: "imap-smtp".into(),
                    secret_id: config.credential_secret_id.clone(),
                    scope: config.credential_scope.clone(),
                    granted_scopes: BTreeSet::from([
                        "mail.message.read".into(),
                        "mail.message.organize".into(),
                        "mail.message.send".into(),
                    ]),
                    expires_at: None,
                })
                .await?;
            (
                Arc::new(ImapSmtpMailConnector::new(
                    config,
                    configured_vault.clone(),
                )?),
                "IMAP/SMTP Mail",
                false,
                None,
            )
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
            (fake, "Fake Mail", true, None)
        }
        MailConnectorMode::Unconfigured => match &vault {
            Some(vault) => {
                let mail = Arc::new(ManagedImapSmtpMailConnector::new());
                let store = SqliteImapSmtpMailAccountStore::from_storage(storage).await?;
                let manager = Arc::new(ImapSmtpMailAccountManager::new(
                    store,
                    vault.clone(),
                    mail.clone(),
                ));
                manager.load_accounts(&scope).await?;
                (mail, "Managed IMAP/SMTP Mail", false, Some(manager))
            }
            None => (
                Arc::new(FakeMailConnector::new()),
                "Unconfigured Mail",
                true,
                None,
            ),
        },
    };
    let runtime = Arc::new(ConnectorRuntime::new_with_ledger(
        vault.as_deref().cloned(),
        ledger,
        256 * 1024,
    )?);
    runtime
        .register(
            MailConnectorTransport::descriptor(display_name, deterministic),
            Arc::new(MailConnectorTransport::new(mail)),
        )
        .await?;
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
    Ok(Some(ResolvedConnectorFoundation {
        tools,
        actions,
        account_manager,
    }))
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
    trusted_key: Option<&SecretMaterial>,
    trusted_root: Option<&Path>,
) -> anyhow::Result<Option<Arc<CredentialVault>>> {
    if let Some(key) = trusted_key {
        let root = trusted_root
            .ok_or_else(|| anyhow::anyhow!("trusted Credential Vault root is unavailable"))?;
        let store = Arc::new(
            agent_runtime::credential_file::EncryptedFileSecretStore::new_borrowed(root, key)?,
        );
        let metadata =
            agent_runtime::credential_sqlite::SqliteCredentialMetadataStore::from_storage(storage)
                .await?;
        return Ok(Some(Arc::new(CredentialVault::new_persistent(
            store, metadata,
        ))));
    }
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
    Ok(Some(Arc::new(CredentialVault::new_persistent(
        store, metadata,
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn connector_mode(values: &[(&str, &str)]) -> anyhow::Result<MailConnectorMode> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        mail_connector_mode_from_lookup(|name| values.get(name).cloned())
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

    #[tokio::test]
    async fn trusted_vault_key_uses_private_root_and_resumes_without_plaintext() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("credentials");
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let scope = CredentialScope {
            app_id: "com.example.secretary".into(),
            tenant_id: "local".into(),
            user_id: "local-user".into(),
        };
        let secret_id =
            agent_runtime::credential::SecretId::parse("mail.primary.password").unwrap();
        let marker = "trusted-vault-credential-marker";
        let first_key = SecretMaterial::new(vec![7; 32]).unwrap();
        let first = resolve_credential_vault(&storage, Some(&first_key), Some(&root))
            .await
            .unwrap()
            .unwrap();
        first
            .configure_connector_account(
                ConnectorAccount {
                    account_id: "primary".into(),
                    connector_id: "agentweave.connector.mail.imap-smtp".into(),
                    provider_id: "imap-smtp".into(),
                    secret_id: secret_id.clone(),
                    scope: scope.clone(),
                    granted_scopes: BTreeSet::from(["mail.message.read".into()]),
                    expires_at: None,
                },
                SecretMaterial::new(marker).unwrap(),
            )
            .await
            .unwrap();
        drop(first);

        let envelopes = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(envelopes.len(), 1);
        assert!(
            !envelopes[0]
                .windows(marker.len())
                .any(|value| value == marker.as_bytes())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        let second_key = SecretMaterial::new(vec![7; 32]).unwrap();
        let resumed = resolve_credential_vault(&storage, Some(&second_key), Some(&root))
            .await
            .unwrap()
            .unwrap();
        assert!(
            resumed
                .connector_credential_configured(
                    &scope,
                    "agentweave.connector.mail.imap-smtp",
                    "primary",
                )
                .await
                .unwrap()
        );
    }
}
