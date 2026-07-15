use crate::credential::{
    ConnectorAccount, CredentialScope, CredentialVault, SecretId, SecretMaterial,
};
use crate::mail::*;
use crate::mail_imap_smtp::{ImapSmtpMailConfig, ImapSmtpMailConnector};
use crate::storage::Storage;
use async_trait::async_trait;
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

pub const IMAP_SMTP_CONNECTOR_ID: &str = "agentweave.connector.mail.imap-smtp";
pub const IMAP_SMTP_PROVIDER_ID: &str = "imap-smtp";

fn granted_scopes() -> BTreeSet<String> {
    BTreeSet::from([
        "mail.message.read".into(),
        "mail.message.organize".into(),
        "mail.message.send".into(),
    ])
}

#[derive(Clone)]
pub struct SqliteImapSmtpMailAccountStore {
    pool: SqlitePool,
}

impl SqliteImapSmtpMailAccountStore {
    pub async fn from_storage(storage: &Storage) -> anyhow::Result<Self> {
        let store = Self {
            pool: storage.pool().clone(),
        };
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS imap_smtp_mail_accounts (
                app_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                config_json TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(app_id, tenant_id, user_id, account_id)
            )"#,
        )
        .execute(&store.pool)
        .await?;
        Ok(store)
    }

    pub async fn list(&self, scope: &CredentialScope) -> anyhow::Result<Vec<ImapSmtpMailConfig>> {
        scope.validate()?;
        let rows = sqlx::query(
            "SELECT account_id, config_json FROM imap_smtp_mail_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? ORDER BY account_id",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| config_from_row(scope, row))
            .collect()
    }

    pub async fn get(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<Option<ImapSmtpMailConfig>> {
        validate_account_id(account_id)?;
        scope.validate()?;
        let row = sqlx::query(
            "SELECT account_id, config_json FROM imap_smtp_mail_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| config_from_row(scope, row)).transpose()
    }

    pub async fn upsert(&self, config: &ImapSmtpMailConfig) -> anyhow::Result<()> {
        config.validate()?;
        let json = serde_json::to_string(config)?;
        sqlx::query(
            r#"INSERT INTO imap_smtp_mail_accounts(app_id, tenant_id, user_id, account_id, config_json, updated_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(app_id, tenant_id, user_id, account_id) DO UPDATE SET config_json = excluded.config_json, updated_at = excluded.updated_at"#,
        )
        .bind(&config.credential_scope.app_id)
        .bind(&config.credential_scope.tenant_id)
        .bind(&config.credential_scope.user_id)
        .bind(&config.account.id)
        .bind(json)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, scope: &CredentialScope, account_id: &str) -> anyhow::Result<bool> {
        validate_account_id(account_id)?;
        scope.validate()?;
        let result = sqlx::query(
            "DELETE FROM imap_smtp_mail_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

fn config_from_row(
    expected_scope: &CredentialScope,
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<ImapSmtpMailConfig> {
    let account_id: String = row.try_get("account_id")?;
    let config: ImapSmtpMailConfig = serde_json::from_str(row.try_get("config_json")?)?;
    config.validate()?;
    anyhow::ensure!(
        &config.credential_scope == expected_scope && config.account.id == account_id,
        "persisted mail account identity does not match its storage scope"
    );
    Ok(config)
}

#[derive(Clone, Default)]
pub struct ManagedImapSmtpMailConnector {
    connectors: Arc<RwLock<BTreeMap<String, Arc<ImapSmtpMailConnector>>>>,
    preview_accounts: Arc<RwLock<BTreeMap<String, String>>>,
}

impl ManagedImapSmtpMailConnector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert_account(
        &self,
        config: ImapSmtpMailConfig,
        vault: Arc<CredentialVault>,
    ) -> anyhow::Result<bool> {
        let account_id = config.account.id.clone();
        let connector = Arc::new(ImapSmtpMailConnector::new(config, vault)?);
        Ok(self.upsert_connector(account_id, connector))
    }

    fn upsert_connector(&self, account_id: String, connector: Arc<ImapSmtpMailConnector>) -> bool {
        self.connectors
            .write()
            .expect("managed mail connector lock poisoned")
            .insert(account_id, connector)
            .is_some()
    }

    pub fn remove_account(&self, account_id: &str) -> bool {
        let removed = self
            .connectors
            .write()
            .expect("managed mail connector lock poisoned")
            .remove(account_id)
            .is_some();
        if removed {
            self.preview_accounts
                .write()
                .expect("managed mail preview lock poisoned")
                .retain(|_, owner| owner != account_id);
        }
        removed
    }

    pub fn contains_account(&self, account_id: &str) -> bool {
        self.connectors
            .read()
            .expect("managed mail connector lock poisoned")
            .contains_key(account_id)
    }

    fn connector(&self, account_id: &str) -> MailResult<Arc<ImapSmtpMailConnector>> {
        self.connectors
            .read()
            .expect("managed mail connector lock poisoned")
            .get(account_id)
            .cloned()
            .ok_or_else(|| MailError::NotFound(account_id.to_string()))
    }
}

macro_rules! dispatch_request {
    ($self:ident, $request:ident, $method:ident) => {{
        let connector = $self.connector(&$request.account_id)?;
        connector.$method($request).await
    }};
}

#[async_trait]
impl MailConnector for ManagedImapSmtpMailConnector {
    async fn list_accounts(&self) -> MailResult<Vec<MailAccount>> {
        let connectors = self
            .connectors
            .read()
            .expect("managed mail connector lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut accounts = Vec::with_capacity(connectors.len());
        for connector in connectors {
            accounts.extend(connector.list_accounts().await?);
        }
        accounts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(accounts)
    }

    async fn account_status(&self, account_id: &str) -> MailResult<MailAccountStatus> {
        self.connector(account_id)?.account_status(account_id).await
    }

    async fn request_connect(
        &self,
        request: MailAccountActionRequest,
    ) -> MailResult<MailAccountStatus> {
        dispatch_request!(self, request, request_connect)
    }

    async fn disconnect(&self, request: MailAccountActionRequest) -> MailResult<MailAccountStatus> {
        dispatch_request!(self, request, disconnect)
    }

    async fn list_mailboxes(&self, account_id: &str) -> MailResult<Vec<Mailbox>> {
        self.connector(account_id)?.list_mailboxes(account_id).await
    }

    async fn list_threads(&self, request: ListThreadsRequest) -> MailResult<Page<MailThread>> {
        dispatch_request!(self, request, list_threads)
    }

    async fn get_thread(&self, request: GetThreadRequest) -> MailResult<MailThreadDetail> {
        dispatch_request!(self, request, get_thread)
    }

    async fn search_messages(
        &self,
        request: SearchMessagesRequest,
    ) -> MailResult<Page<MailMessageSummary>> {
        dispatch_request!(self, request, search_messages)
    }

    async fn get_message(&self, request: GetMessageRequest) -> MailResult<MailMessage> {
        dispatch_request!(self, request, get_message)
    }

    async fn read_body_part(&self, request: ReadBodyPartRequest) -> MailResult<BodyPartContent> {
        dispatch_request!(self, request, read_body_part)
    }

    async fn read_attachment(&self, request: ReadAttachmentRequest) -> MailResult<AttachmentChunk> {
        dispatch_request!(self, request, read_attachment)
    }

    async fn set_read_state(&self, request: SetReadStateRequest) -> MailResult<MailMessageSummary> {
        dispatch_request!(self, request, set_read_state)
    }

    async fn archive_message(
        &self,
        request: ArchiveMessageRequest,
    ) -> MailResult<MailMessageSummary> {
        dispatch_request!(self, request, archive_message)
    }

    async fn move_message(&self, request: MoveMessageRequest) -> MailResult<MailMessageSummary> {
        dispatch_request!(self, request, move_message)
    }

    async fn create_draft(&self, request: CreateDraftRequest) -> MailResult<MailDraft> {
        dispatch_request!(self, request, create_draft)
    }

    async fn create_reply_draft(&self, request: CreateReplyDraftRequest) -> MailResult<MailDraft> {
        dispatch_request!(self, request, create_reply_draft)
    }

    async fn create_forward_draft(
        &self,
        request: CreateForwardDraftRequest,
    ) -> MailResult<MailDraft> {
        dispatch_request!(self, request, create_forward_draft)
    }

    async fn get_draft(&self, request: GetDraftRequest) -> MailResult<MailDraft> {
        dispatch_request!(self, request, get_draft)
    }

    async fn update_draft(&self, request: UpdateDraftRequest) -> MailResult<MailDraft> {
        dispatch_request!(self, request, update_draft)
    }

    async fn delete_draft(&self, request: DeleteDraftRequest) -> MailResult<()> {
        dispatch_request!(self, request, delete_draft)
    }

    async fn preview_send(&self, request: PreviewSendRequest) -> MailResult<SendPreview> {
        let account_id = request.account_id.clone();
        let preview = self.connector(&account_id)?.preview_send(request).await?;
        self.preview_accounts
            .write()
            .expect("managed mail preview lock poisoned")
            .insert(preview.id.clone(), account_id);
        Ok(preview)
    }

    async fn send_approved(&self, request: ApprovedSendRequest) -> MailResult<DeliveryReceipt> {
        let account_id = self
            .preview_accounts
            .read()
            .expect("managed mail preview lock poisoned")
            .get(&request.preview_id)
            .cloned()
            .ok_or_else(|| MailError::NotFound(request.preview_id.clone()))?;
        self.connector(&account_id)?.send_approved(request).await
    }

    async fn delivery_status(&self, request: DeliveryStatusRequest) -> MailResult<DeliveryReceipt> {
        dispatch_request!(self, request, delivery_status)
    }
}

#[derive(Clone)]
pub struct ImapSmtpMailAccountManager {
    store: SqliteImapSmtpMailAccountStore,
    vault: Arc<CredentialVault>,
    connector: Arc<ManagedImapSmtpMailConnector>,
}

impl ImapSmtpMailAccountManager {
    pub fn new(
        store: SqliteImapSmtpMailAccountStore,
        vault: Arc<CredentialVault>,
        connector: Arc<ManagedImapSmtpMailConnector>,
    ) -> Self {
        Self {
            store,
            vault,
            connector,
        }
    }

    pub async fn load_accounts(
        &self,
        scope: &CredentialScope,
    ) -> anyhow::Result<Vec<ImapSmtpMailConfig>> {
        let configs = self.store.list(scope).await?;
        let mut prepared = Vec::with_capacity(configs.len());
        for config in &configs {
            let account = self
                .vault
                .connector_account(scope, &config.account.id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "credential metadata is unavailable for mail account {}",
                        config.account.id
                    )
                })?;
            anyhow::ensure!(
                account.connector_id == IMAP_SMTP_CONNECTOR_ID
                    && account.secret_id == config.credential_secret_id,
                "mail account credential metadata is inconsistent"
            );
            anyhow::ensure!(
                self.vault
                    .connector_credential_configured(
                        scope,
                        IMAP_SMTP_CONNECTOR_ID,
                        &config.account.id,
                    )
                    .await?,
                "mail account credential is unavailable"
            );
            prepared.push((
                config.account.id.clone(),
                Arc::new(ImapSmtpMailConnector::new(
                    config.clone(),
                    self.vault.clone(),
                )?),
            ));
        }
        for (account_id, connector) in prepared {
            self.connector.upsert_connector(account_id, connector);
        }
        Ok(configs)
    }

    pub async fn configure(
        &self,
        mut config: ImapSmtpMailConfig,
        password: SecretMaterial,
    ) -> anyhow::Result<ImapSmtpMailConfig> {
        config.credential_scope.validate()?;
        validate_account_id(&config.account.id)?;
        config.credential_secret_id = mail_secret_id(&config.credential_scope, &config.account.id)?;
        config.validate()?;
        let prepared = Arc::new(ImapSmtpMailConnector::new(
            config.clone(),
            self.vault.clone(),
        )?);

        let previous = self
            .store
            .get(&config.credential_scope, &config.account.id)
            .await?;
        self.store.upsert(&config).await?;
        let account = ConnectorAccount {
            account_id: config.account.id.clone(),
            connector_id: IMAP_SMTP_CONNECTOR_ID.into(),
            provider_id: IMAP_SMTP_PROVIDER_ID.into(),
            secret_id: config.credential_secret_id.clone(),
            scope: config.credential_scope.clone(),
            granted_scopes: granted_scopes(),
            expires_at: None,
        };
        if let Err(error) = self
            .vault
            .configure_connector_account(account, password)
            .await
        {
            let compensation = match previous {
                Some(previous) => self.store.upsert(&previous).await,
                None => self
                    .store
                    .delete(&config.credential_scope, &config.account.id)
                    .await
                    .map(|_| ()),
            };
            return match compensation {
                Ok(()) => Err(error),
                Err(compensation) => Err(error.context(format!(
                    "mail configuration compensation failed: {compensation:#}"
                ))),
            };
        }
        self.connector
            .upsert_connector(config.account.id.clone(), prepared);
        Ok(config)
    }

    pub async fn delete(&self, scope: &CredentialScope, account_id: &str) -> anyhow::Result<bool> {
        scope.validate()?;
        validate_account_id(account_id)?;
        let previous = self.store.get(scope, account_id).await?;
        let config_deleted = self.store.delete(scope, account_id).await?;
        let connector_deleted = self.connector.remove_account(account_id);
        let credential_deleted = match self.vault.delete_connector_account(scope, account_id).await
        {
            Ok(deleted) => deleted,
            Err(error) => {
                let compensation = match previous {
                    Some(config) => {
                        async {
                            self.store.upsert(&config).await?;
                            if connector_deleted {
                                self.connector.upsert_account(config, self.vault.clone())?;
                            }
                            Ok::<_, anyhow::Error>(())
                        }
                        .await
                    }
                    None => Ok(()),
                };
                return match compensation {
                    Ok(()) => Err(error),
                    Err(compensation) => Err(error.context(format!(
                        "mail deletion compensation failed: {compensation:#}"
                    ))),
                };
            }
        };
        Ok(config_deleted || connector_deleted || credential_deleted)
    }

    pub async fn list(&self, scope: &CredentialScope) -> anyhow::Result<Vec<ImapSmtpMailConfig>> {
        self.store.list(scope).await
    }

    pub async fn get(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<Option<ImapSmtpMailConfig>> {
        self.store.get(scope, account_id).await
    }

    pub async fn credential_configured(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<bool> {
        self.vault
            .connector_credential_configured(scope, IMAP_SMTP_CONNECTOR_ID, account_id)
            .await
    }
}

pub fn mail_secret_id(scope: &CredentialScope, account_id: &str) -> anyhow::Result<SecretId> {
    scope.validate()?;
    validate_account_id(account_id)?;
    let identity = serde_json::to_vec(&(
        "agentweave.mail.imap-smtp.v1",
        &scope.app_id,
        &scope.tenant_id,
        &scope.user_id,
        account_id,
    ))?;
    SecretId::parse(&format!(
        "mail.imap-smtp.{}",
        hex::encode(Sha256::digest(identity))
    ))
}

fn validate_account_id(account_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!account_id.trim().is_empty(), "account id is required");
    anyhow::ensure!(account_id.len() <= 255, "account id is too long");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{InMemorySecretStore, SecretStore};
    use crate::credential_sqlite::SqliteCredentialMetadataStore;
    use crate::mail_imap_smtp::MailTlsMode;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct DeleteFailingSecretStore {
        inner: InMemorySecretStore,
        fail_delete: AtomicBool,
    }

    #[async_trait]
    impl SecretStore for DeleteFailingSecretStore {
        async fn save(
            &self,
            scope: &CredentialScope,
            secret_id: &SecretId,
            value: SecretMaterial,
        ) -> anyhow::Result<()> {
            self.inner.save(scope, secret_id, value).await
        }

        async fn load(
            &self,
            scope: &CredentialScope,
            secret_id: &SecretId,
        ) -> anyhow::Result<Option<SecretMaterial>> {
            self.inner.load(scope, secret_id).await
        }

        async fn delete(
            &self,
            scope: &CredentialScope,
            secret_id: &SecretId,
        ) -> anyhow::Result<bool> {
            if self.fail_delete.load(Ordering::SeqCst) {
                anyhow::bail!("injected secret deletion failure");
            }
            self.inner.delete(scope, secret_id).await
        }

        async fn rotate(
            &self,
            scope: &CredentialScope,
            secret_id: &SecretId,
            value: SecretMaterial,
        ) -> anyhow::Result<()> {
            self.inner.rotate(scope, secret_id, value).await
        }
    }

    fn scope(app_id: &str) -> CredentialScope {
        CredentialScope {
            app_id: app_id.into(),
            tenant_id: "local".into(),
            user_id: "user".into(),
        }
    }

    fn config(account_id: &str, scope: CredentialScope) -> ImapSmtpMailConfig {
        ImapSmtpMailConfig {
            account: MailAccount {
                id: account_id.into(),
                display_name: format!("Account {account_id}"),
                primary_address: MailAddress {
                    name: None,
                    address: format!("{account_id}@example.test"),
                },
                addresses: vec![],
                provider_reference: Some(ProviderReference {
                    provider: IMAP_SMTP_PROVIDER_ID.into(),
                    id: account_id.into(),
                }),
            },
            credential_scope: scope,
            credential_secret_id: SecretId::parse("caller.supplied.is.replaced").unwrap(),
            imap_host: "127.0.0.1".into(),
            imap_port: 1143,
            imap_tls: MailTlsMode::None,
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_tls: MailTlsMode::None,
            username: format!("{account_id}@example.test"),
            archive_mailbox: Some("Archive".into()),
            sent_mailbox: Some("Sent".into()),
            drafts_mailbox: Some("Drafts".into()),
            trash_mailbox: Some("Trash".into()),
            allow_insecure_localhost: true,
            connect_timeout_seconds: 2,
            operation_timeout_seconds: 3,
        }
    }

    fn draft_content(subject: &str) -> DraftContent {
        DraftContent {
            to: vec![MailAddress {
                name: None,
                address: "recipient@example.test".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: subject.into(),
            body: OutgoingBody {
                plain_text: "hello".into(),
                html: None,
            },
            attachments: vec![],
            reply_context: None,
            forward_context: None,
        }
    }

    async fn manager(
        storage: &Storage,
        secrets: Arc<InMemorySecretStore>,
    ) -> (
        ImapSmtpMailAccountManager,
        Arc<ManagedImapSmtpMailConnector>,
        Arc<CredentialVault>,
    ) {
        manager_with_store(storage, secrets).await
    }

    async fn manager_with_store(
        storage: &Storage,
        secrets: Arc<dyn SecretStore>,
    ) -> (
        ImapSmtpMailAccountManager,
        Arc<ManagedImapSmtpMailConnector>,
        Arc<CredentialVault>,
    ) {
        let metadata = SqliteCredentialMetadataStore::from_storage(storage)
            .await
            .unwrap();
        let vault = Arc::new(CredentialVault::new_persistent(secrets, metadata));
        let connector = Arc::new(ManagedImapSmtpMailConnector::new());
        let store = SqliteImapSmtpMailAccountStore::from_storage(storage)
            .await
            .unwrap();
        (
            ImapSmtpMailAccountManager::new(store, vault.clone(), connector.clone()),
            connector,
            vault,
        )
    }

    #[tokio::test]
    async fn store_is_scope_bound_and_persists_only_non_secret_config() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let store = SqliteImapSmtpMailAccountStore::from_storage(&storage)
            .await
            .unwrap();
        let value = config("primary", scope("app-a"));
        store.upsert(&value).await.unwrap();

        assert_eq!(store.list(&scope("app-a")).await.unwrap(), vec![value]);
        assert!(store.list(&scope("app-b")).await.unwrap().is_empty());
        let persisted: String = sqlx::query_scalar(
            "SELECT config_json FROM imap_smtp_mail_accounts WHERE account_id = 'primary'",
        )
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(!persisted.contains("password-value"));
        assert!(!persisted.to_ascii_lowercase().contains("password\":"));
    }

    #[tokio::test]
    async fn manager_configures_updates_reloads_deletes_and_dispatches_by_account() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let (manager, connector, vault) = manager(&storage, secrets.clone()).await;
        let account_scope = scope("app-a");

        let primary = manager
            .configure(
                config("primary", account_scope.clone()),
                SecretMaterial::new("password-value").unwrap(),
            )
            .await
            .unwrap();
        manager
            .configure(
                config("secondary", account_scope.clone()),
                SecretMaterial::new("secondary-password").unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            primary.credential_secret_id.as_str(),
            "caller.supplied.is.replaced"
        );
        let account = vault
            .connector_account(&account_scope, "primary")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(account.connector_id, IMAP_SMTP_CONNECTOR_ID);
        assert_eq!(account.granted_scopes, granted_scopes());
        assert!(
            manager
                .credential_configured(&account_scope, "primary")
                .await
                .unwrap()
        );

        let draft = connector
            .create_draft(CreateDraftRequest {
                account_id: "secondary".into(),
                content: draft_content("secondary draft"),
            })
            .await
            .unwrap();
        assert_eq!(draft.account_id, "secondary");
        assert!(matches!(
            connector.account_status("missing").await,
            Err(MailError::NotFound(value)) if value == "missing"
        ));

        let mut updated = config("primary", account_scope.clone());
        updated.username = "updated@example.test".into();
        manager
            .configure(updated, SecretMaterial::new("new-password").unwrap())
            .await
            .unwrap();
        assert_eq!(
            manager
                .get(&account_scope, "primary")
                .await
                .unwrap()
                .unwrap()
                .username,
            "updated@example.test"
        );
        let leased = vault
            .lease_for_connector(
                &account_scope,
                IMAP_SMTP_CONNECTOR_ID,
                "primary",
                &BTreeSet::from(["mail.message.read".into()]),
            )
            .await
            .unwrap();
        assert_eq!(leased.expose_bytes(), b"new-password");

        let resumed_connector = Arc::new(ManagedImapSmtpMailConnector::new());
        let resumed_store = SqliteImapSmtpMailAccountStore::from_storage(&storage)
            .await
            .unwrap();
        let resumed = ImapSmtpMailAccountManager::new(
            resumed_store,
            vault.clone(),
            resumed_connector.clone(),
        );
        assert_eq!(
            resumed.load_accounts(&account_scope).await.unwrap().len(),
            2
        );
        assert!(resumed_connector.contains_account("primary"));

        assert!(resumed.delete(&account_scope, "primary").await.unwrap());
        assert!(!resumed_connector.contains_account("primary"));
        assert!(
            secrets
                .load(&account_scope, &primary.credential_secret_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !resumed
                .credential_configured(&account_scope, "primary")
                .await
                .unwrap()
        );
        assert!(!resumed.delete(&account_scope, "primary").await.unwrap());
        assert!(resumed_connector.contains_account("secondary"));
    }

    #[tokio::test]
    async fn configure_compensates_config_and_secret_when_metadata_fails() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let (manager, connector, vault) = manager(&storage, secrets.clone()).await;
        let account_scope = scope("app-a");
        let secret_id = mail_secret_id(&account_scope, "primary").unwrap();
        vault
            .register_account(ConnectorAccount {
                account_id: "primary".into(),
                connector_id: IMAP_SMTP_CONNECTOR_ID.into(),
                provider_id: IMAP_SMTP_PROVIDER_ID.into(),
                secret_id: secret_id.clone(),
                scope: account_scope.clone(),
                granted_scopes: granted_scopes(),
                expires_at: None,
            })
            .unwrap();
        sqlx::query("DROP TABLE connector_accounts")
            .execute(storage.pool())
            .await
            .unwrap();

        let error = manager
            .configure(
                config("primary", account_scope.clone()),
                SecretMaterial::new("must-be-removed").unwrap(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("connector_accounts"));
        assert!(
            manager
                .get(&account_scope, "primary")
                .await
                .unwrap()
                .is_none()
        );
        assert!(!connector.contains_account("primary"));
        assert!(
            secrets
                .load(&account_scope, &secret_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn delete_restores_config_and_connector_when_secret_deletion_fails() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let secrets = Arc::new(DeleteFailingSecretStore {
            inner: InMemorySecretStore::default(),
            fail_delete: AtomicBool::new(false),
        });
        let (manager, connector, vault) = manager_with_store(&storage, secrets.clone()).await;
        let account_scope = scope("app-a");
        manager
            .configure(
                config("primary", account_scope.clone()),
                SecretMaterial::new("password-value").unwrap(),
            )
            .await
            .unwrap();
        secrets.fail_delete.store(true, Ordering::SeqCst);

        let error = manager.delete(&account_scope, "primary").await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("injected secret deletion failure")
        );
        assert!(
            manager
                .get(&account_scope, "primary")
                .await
                .unwrap()
                .is_some()
        );
        assert!(connector.contains_account("primary"));
        assert!(
            vault
                .connector_credential_configured(&account_scope, IMAP_SMTP_CONNECTOR_ID, "primary",)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn startup_loading_is_fail_closed_when_any_secret_is_missing() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let (manager, connector, vault) = manager(&storage, secrets).await;
        let account_scope = scope("app-a");
        let mut valid = config("valid", account_scope.clone());
        valid.credential_secret_id = mail_secret_id(&account_scope, "valid").unwrap();
        let mut missing = config("missing", account_scope.clone());
        missing.credential_secret_id = mail_secret_id(&account_scope, "missing").unwrap();
        manager.store.upsert(&valid).await.unwrap();
        manager.store.upsert(&missing).await.unwrap();
        vault
            .configure_connector_account(
                ConnectorAccount {
                    account_id: "valid".into(),
                    connector_id: IMAP_SMTP_CONNECTOR_ID.into(),
                    provider_id: IMAP_SMTP_PROVIDER_ID.into(),
                    secret_id: valid.credential_secret_id.clone(),
                    scope: account_scope.clone(),
                    granted_scopes: granted_scopes(),
                    expires_at: None,
                },
                SecretMaterial::new("present").unwrap(),
            )
            .await
            .unwrap();
        vault
            .register_account_persistent(ConnectorAccount {
                account_id: "missing".into(),
                connector_id: IMAP_SMTP_CONNECTOR_ID.into(),
                provider_id: IMAP_SMTP_PROVIDER_ID.into(),
                secret_id: missing.credential_secret_id.clone(),
                scope: account_scope.clone(),
                granted_scopes: granted_scopes(),
                expires_at: None,
            })
            .await
            .unwrap();

        assert!(
            manager
                .load_accounts(&account_scope)
                .await
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
        assert!(!connector.contains_account("valid"));
        assert!(!connector.contains_account("missing"));
    }

    #[tokio::test]
    async fn startup_loading_rejects_connector_metadata_mismatch() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let (manager, connector, vault) =
            manager(&storage, Arc::new(InMemorySecretStore::default())).await;
        let account_scope = scope("app-a");
        let mut value = config("primary", account_scope.clone());
        value.credential_secret_id = mail_secret_id(&account_scope, "primary").unwrap();
        manager.store.upsert(&value).await.unwrap();
        vault
            .configure_connector_account(
                ConnectorAccount {
                    account_id: "primary".into(),
                    connector_id: "different-connector".into(),
                    provider_id: IMAP_SMTP_PROVIDER_ID.into(),
                    secret_id: value.credential_secret_id,
                    scope: account_scope.clone(),
                    granted_scopes: granted_scopes(),
                    expires_at: None,
                },
                SecretMaterial::new("present").unwrap(),
            )
            .await
            .unwrap();

        let error = manager.load_accounts(&account_scope).await.unwrap_err();
        assert!(error.to_string().contains("inconsistent"));
        assert!(!connector.contains_account("primary"));
    }
}
