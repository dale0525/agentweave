use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct SecretId(String);

impl SecretId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !value.is_empty()
                && value.len() <= 255
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character)),
            "invalid opaque secret id"
        );
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct CredentialScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
}

impl CredentialScope {
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [&self.app_id, &self.tenant_id, &self.user_id] {
            anyhow::ensure!(
                !value.trim().is_empty(),
                "credential scope values are required"
            );
            anyhow::ensure!(value.len() <= 255, "credential scope value is too long");
        }
        Ok(())
    }
}

pub struct SecretMaterial(Vec<u8>);

impl SecretMaterial {
    pub fn new(value: impl Into<Vec<u8>>) -> anyhow::Result<Self> {
        let value = value.into();
        anyhow::ensure!(!value.is_empty(), "secret material cannot be empty");
        anyhow::ensure!(value.len() <= 64 * 1024, "secret material is too large");
        Ok(Self(value))
    }

    pub(crate) fn expose_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretMaterial([REDACTED])")
    }
}

impl Drop for SecretMaterial {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn save(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
        value: SecretMaterial,
    ) -> anyhow::Result<()>;
    async fn load(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
    ) -> anyhow::Result<Option<SecretMaterial>>;
    async fn delete(&self, scope: &CredentialScope, secret_id: &SecretId) -> anyhow::Result<bool>;
    async fn rotate(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
        value: SecretMaterial,
    ) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct InMemorySecretStore {
    values: Mutex<BTreeMap<(CredentialScope, SecretId), Vec<u8>>>,
}

#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn save(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
        value: SecretMaterial,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        let mut values = self.values.lock().expect("secret store lock poisoned");
        anyhow::ensure!(
            !values.contains_key(&(scope.clone(), secret_id.clone())),
            "secret already exists"
        );
        values.insert(
            (scope.clone(), secret_id.clone()),
            value.expose_bytes().to_vec(),
        );
        Ok(())
    }

    async fn load(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
    ) -> anyhow::Result<Option<SecretMaterial>> {
        scope.validate()?;
        self.values
            .lock()
            .expect("secret store lock poisoned")
            .get(&(scope.clone(), secret_id.clone()))
            .cloned()
            .map(SecretMaterial::new)
            .transpose()
    }

    async fn delete(&self, scope: &CredentialScope, secret_id: &SecretId) -> anyhow::Result<bool> {
        scope.validate()?;
        Ok(self
            .values
            .lock()
            .expect("secret store lock poisoned")
            .remove(&(scope.clone(), secret_id.clone()))
            .is_some())
    }

    async fn rotate(
        &self,
        scope: &CredentialScope,
        secret_id: &SecretId,
        value: SecretMaterial,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        let mut values = self.values.lock().expect("secret store lock poisoned");
        let stored = values
            .get_mut(&(scope.clone(), secret_id.clone()))
            .ok_or_else(|| anyhow::anyhow!("secret does not exist"))?;
        stored.fill(0);
        *stored = value.expose_bytes().to_vec();
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorAccount {
    pub account_id: String,
    pub connector_id: String,
    pub provider_id: String,
    pub secret_id: SecretId,
    pub scope: CredentialScope,
    pub granted_scopes: BTreeSet<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthAuthorizationState {
    pub state_id: String,
    pub connector_id: String,
    pub account_id: String,
    pub pkce_verifier_secret_id: SecretId,
    pub redirect_uri: String,
    pub requested_scopes: BTreeSet<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OAuthTokenRecord {
    pub account_id: String,
    pub connector_id: String,
    pub provider_id: String,
    pub access_token_secret_id: SecretId,
    pub refresh_token_secret_id: Option<SecretId>,
    pub granted_scopes: BTreeSet<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct CredentialVault {
    store: Arc<dyn SecretStore>,
    accounts: Arc<Mutex<BTreeMap<(CredentialScope, String), ConnectorAccount>>>,
    metadata: Option<crate::credential_sqlite::SqliteCredentialMetadataStore>,
}

impl CredentialVault {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            accounts: Arc::new(Mutex::new(BTreeMap::new())),
            metadata: None,
        }
    }

    pub fn new_persistent(
        store: Arc<dyn SecretStore>,
        metadata: crate::credential_sqlite::SqliteCredentialMetadataStore,
    ) -> Self {
        Self {
            store,
            accounts: Arc::new(Mutex::new(BTreeMap::new())),
            metadata: Some(metadata),
        }
    }

    pub fn register_account(&self, account: ConnectorAccount) -> anyhow::Result<()> {
        account.scope.validate()?;
        anyhow::ensure!(
            !account.account_id.trim().is_empty(),
            "account id is required"
        );
        anyhow::ensure!(
            !account.connector_id.trim().is_empty(),
            "connector id is required"
        );
        self.accounts
            .lock()
            .expect("credential account lock poisoned")
            .insert((account.scope.clone(), account.account_id.clone()), account);
        Ok(())
    }

    pub async fn register_account_persistent(
        &self,
        account: ConnectorAccount,
    ) -> anyhow::Result<()> {
        self.register_account(account.clone())?;
        if let Some(metadata) = &self.metadata {
            metadata.upsert_account(&account).await?;
        }
        Ok(())
    }

    pub async fn begin_oauth_authorization(
        &self,
        scope: &CredentialScope,
        state: OAuthAuthorizationState,
        pkce_verifier: SecretMaterial,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        anyhow::ensure!(
            state.expires_at > Utc::now(),
            "OAuth state is already expired"
        );
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent credential metadata is unavailable"))?;
        self.store
            .save(scope, &state.pkce_verifier_secret_id, pkce_verifier)
            .await?;
        if let Err(error) = metadata.save_oauth_state(scope, &state).await {
            let _ = self
                .store
                .delete(scope, &state.pkce_verifier_secret_id)
                .await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn consume_oauth_authorization(
        &self,
        scope: &CredentialScope,
        state_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<(OAuthAuthorizationState, SecretMaterial)> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent credential metadata is unavailable"))?;
        let state = metadata.consume_oauth_state(scope, state_id, now).await?;
        let verifier = self
            .store
            .load(scope, &state.pkce_verifier_secret_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OAuth PKCE verifier is unavailable"))?;
        self.store
            .delete(scope, &state.pkce_verifier_secret_id)
            .await?;
        Ok((state, verifier))
    }

    pub async fn save_oauth_tokens(
        &self,
        scope: &CredentialScope,
        record: OAuthTokenRecord,
        access_token: SecretMaterial,
        refresh_token: Option<SecretMaterial>,
    ) -> anyhow::Result<()> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent credential metadata is unavailable"))?;
        self.store
            .save(scope, &record.access_token_secret_id, access_token)
            .await?;
        if let (Some(secret_id), Some(token)) = (&record.refresh_token_secret_id, refresh_token)
            && let Err(error) = self.store.save(scope, secret_id, token).await
        {
            let _ = self
                .store
                .delete(scope, &record.access_token_secret_id)
                .await;
            return Err(error);
        }
        if let Err(error) = metadata.upsert_oauth_tokens(scope, &record).await {
            let _ = self
                .store
                .delete(scope, &record.access_token_secret_id)
                .await;
            if let Some(secret_id) = &record.refresh_token_secret_id {
                let _ = self.store.delete(scope, secret_id).await;
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn revoke_oauth_account(
        &self,
        scope: &CredentialScope,
        account_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent credential metadata is unavailable"))?;
        let Some(record) = metadata.get_oauth_tokens(scope, account_id).await? else {
            return Ok(false);
        };
        metadata.revoke_oauth_tokens(scope, account_id, now).await?;
        self.store
            .delete(scope, &record.access_token_secret_id)
            .await?;
        if let Some(secret_id) = &record.refresh_token_secret_id {
            self.store.delete(scope, secret_id).await?;
        }
        Ok(true)
    }

    pub async fn lease_for_connector(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        account_id: &str,
        required_scopes: &BTreeSet<String>,
    ) -> anyhow::Result<SecretMaterial> {
        let account = self
            .accounts
            .lock()
            .expect("credential account lock poisoned")
            .get(&(scope.clone(), account_id.to_string()))
            .cloned();
        let account = match account {
            Some(account) => account,
            None => self
                .metadata
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("connector account is unavailable"))?
                .get_account(scope, account_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("connector account is unavailable"))?,
        };
        anyhow::ensure!(
            account.connector_id == connector_id,
            "connector account mismatch"
        );
        anyhow::ensure!(
            required_scopes.is_subset(&account.granted_scopes),
            "connector account lacks required scopes"
        );
        anyhow::ensure!(
            account.expires_at.is_none_or(|expiry| expiry > Utc::now()),
            "connector account authorization expired"
        );
        self.store
            .load(scope, &account.secret_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connector credential is unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(app_id: &str) -> CredentialScope {
        CredentialScope {
            app_id: app_id.into(),
            tenant_id: "tenant".into(),
            user_id: "user".into(),
        }
    }

    #[tokio::test]
    async fn secrets_are_scoped_rotatable_and_redacted() {
        let store = InMemorySecretStore::default();
        let id = SecretId::parse("mail.account.primary").unwrap();
        store
            .save(
                &scope("com.example.a"),
                &id,
                SecretMaterial::new("old").unwrap(),
            )
            .await
            .unwrap();
        assert!(
            store
                .load(&scope("com.example.b"), &id)
                .await
                .unwrap()
                .is_none()
        );
        store
            .rotate(
                &scope("com.example.a"),
                &id,
                SecretMaterial::new("new").unwrap(),
            )
            .await
            .unwrap();
        let loaded = store
            .load(&scope("com.example.a"), &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.expose_bytes(), b"new");
        assert_eq!(format!("{loaded:?}"), "SecretMaterial([REDACTED])");
    }

    #[tokio::test]
    async fn vault_only_leases_exact_connector_account_and_scopes() {
        let store = Arc::new(InMemorySecretStore::default());
        let id = SecretId::parse("mail.account.primary").unwrap();
        let account_scope = scope("com.example.mail");
        store
            .save(
                &account_scope,
                &id,
                SecretMaterial::new("credential").unwrap(),
            )
            .await
            .unwrap();
        let vault = CredentialVault::new(store);
        vault
            .register_account(ConnectorAccount {
                account_id: "primary".into(),
                connector_id: "mail.fake".into(),
                provider_id: "fake".into(),
                secret_id: id,
                scope: account_scope.clone(),
                granted_scopes: BTreeSet::from(["mail.read".into()]),
                expires_at: None,
            })
            .unwrap();

        assert!(
            vault
                .lease_for_connector(
                    &account_scope,
                    "mail.fake",
                    "primary",
                    &BTreeSet::from(["mail.read".into()])
                )
                .await
                .is_ok()
        );
        assert!(
            vault
                .lease_for_connector(&account_scope, "other", "primary", &BTreeSet::new())
                .await
                .is_err()
        );
    }

    #[test]
    fn serialized_metadata_never_contains_secret_material() {
        let account = ConnectorAccount {
            account_id: "primary".into(),
            connector_id: "mail.fake".into(),
            provider_id: "fake".into(),
            secret_id: SecretId::parse("mail.account.primary").unwrap(),
            scope: scope("com.example.mail"),
            granted_scopes: BTreeSet::new(),
            expires_at: None,
        };
        let json = serde_json::to_string(&account).unwrap();
        assert!(json.contains("mail.account.primary"));
        assert!(!json.contains("credential-value"));
    }
}
