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
        let removed = self
            .values
            .lock()
            .expect("secret store lock poisoned")
            .remove(&(scope.clone(), secret_id.clone()));
        if let Some(mut value) = removed {
            value.fill(0);
            Ok(true)
        } else {
            Ok(false)
        }
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

impl Drop for InMemorySecretStore {
    fn drop(&mut self) {
        if let Ok(values) = self.values.get_mut() {
            for value in values.values_mut() {
                value.fill(0);
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorAccount {
    pub account_id: String,
    pub connector_id: String,
    pub credential_id: String,
    pub scope: CredentialScope,
    pub allowed_scopes: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredential {
    pub credential_id: String,
    pub provider_id: String,
    pub provider_subject: String,
    pub access_secret_id: SecretId,
    pub refresh_secret_id: Option<SecretId>,
    pub granted_scopes: BTreeSet<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
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

type ConnectorAccountKey = (CredentialScope, String, String);
type ConnectorAccountMap = BTreeMap<ConnectorAccountKey, ConnectorAccount>;
type ProviderCredentialKey = (CredentialScope, String);
type ProviderCredentialMap = BTreeMap<ProviderCredentialKey, ProviderCredential>;

#[derive(Default)]
struct CredentialVaultState {
    accounts: ConnectorAccountMap,
    credentials: ProviderCredentialMap,
}

#[derive(Clone)]
pub struct CredentialVault {
    store: Arc<dyn SecretStore>,
    state: Arc<Mutex<CredentialVaultState>>,
    metadata: Option<crate::credential_sqlite::SqliteCredentialMetadataStore>,
}

impl CredentialVault {
    pub fn new(store: Arc<dyn SecretStore>) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(CredentialVaultState::default())),
            metadata: None,
        }
    }

    pub fn new_persistent(
        store: Arc<dyn SecretStore>,
        metadata: crate::credential_sqlite::SqliteCredentialMetadataStore,
    ) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(CredentialVaultState::default())),
            metadata: Some(metadata),
        }
    }

    pub fn register_provider_credential(
        &self,
        scope: &CredentialScope,
        credential: ProviderCredential,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        validate_provider_credential(&credential)?;
        self.state
            .lock()
            .expect("credential vault state lock poisoned")
            .credentials
            .insert(
                (scope.clone(), credential.credential_id.clone()),
                credential,
            );
        Ok(())
    }

    pub async fn register_provider_credential_persistent(
        &self,
        scope: &CredentialScope,
        credential: ProviderCredential,
    ) -> anyhow::Result<()> {
        validate_provider_credential(&credential)?;
        scope.validate()?;
        if let Some(metadata) = &self.metadata {
            metadata.upsert_credential(scope, &credential).await?;
        }
        self.state
            .lock()
            .expect("credential vault state lock poisoned")
            .credentials
            .insert(
                (scope.clone(), credential.credential_id.clone()),
                credential,
            );
        Ok(())
    }

    pub fn register_account(&self, account: ConnectorAccount) -> anyhow::Result<()> {
        validate_connector_account(&account)?;
        let mut state = self
            .state
            .lock()
            .expect("credential vault state lock poisoned");
        let credential = state
            .credentials
            .get(&(account.scope.clone(), account.credential_id.clone()))
            .ok_or_else(|| anyhow::anyhow!("connector account credential is unavailable"))?;
        anyhow::ensure!(
            account.allowed_scopes.is_subset(&credential.granted_scopes),
            "connector account scopes exceed provider credential grant"
        );
        anyhow::ensure!(
            credential.revoked_at.is_none(),
            "connector account credential is revoked"
        );
        state.accounts.insert(
            (
                account.scope.clone(),
                account.connector_id.clone(),
                account.account_id.clone(),
            ),
            account,
        );
        Ok(())
    }

    pub async fn register_account_persistent(
        &self,
        account: ConnectorAccount,
    ) -> anyhow::Result<()> {
        validate_connector_account(&account)?;
        if let Some(metadata) = &self.metadata {
            metadata.upsert_account(&account).await?;
            self.state
                .lock()
                .expect("credential vault state lock poisoned")
                .accounts
                .insert(
                    (
                        account.scope.clone(),
                        account.connector_id.clone(),
                        account.account_id.clone(),
                    ),
                    account,
                );
            Ok(())
        } else {
            self.register_account(account)
        }
    }

    pub async fn list_connector_accounts(
        &self,
        scope: &CredentialScope,
        connector_id: Option<&str>,
    ) -> anyhow::Result<Vec<ConnectorAccount>> {
        scope.validate()?;
        if let Some(metadata) = &self.metadata {
            return metadata.list_accounts(scope, connector_id).await;
        }
        let mut accounts = self
            .state
            .lock()
            .expect("credential vault state lock poisoned")
            .accounts
            .values()
            .filter(|account| {
                account.scope == *scope
                    && connector_id.is_none_or(|expected| account.connector_id == expected)
            })
            .cloned()
            .collect::<Vec<_>>();
        accounts.sort_by(|left, right| {
            (&left.connector_id, &left.account_id).cmp(&(&right.connector_id, &right.account_id))
        });
        Ok(accounts)
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

    pub async fn save_provider_credential(
        &self,
        scope: &CredentialScope,
        credential: ProviderCredential,
        access_secret: SecretMaterial,
        refresh_secret: Option<SecretMaterial>,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        validate_provider_credential(&credential)?;
        anyhow::ensure!(
            credential.refresh_secret_id.is_some() == refresh_secret.is_some(),
            "refresh secret metadata and material must be provided together"
        );
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("persistent credential metadata is unavailable"))?;
        self.store
            .save(scope, &credential.access_secret_id, access_secret)
            .await?;
        if let (Some(secret_id), Some(secret)) = (&credential.refresh_secret_id, refresh_secret)
            && let Err(error) = self.store.save(scope, secret_id, secret).await
        {
            let _ = self.store.delete(scope, &credential.access_secret_id).await;
            return Err(error);
        }
        if let Err(error) = metadata.upsert_credential(scope, &credential).await {
            let _ = self.store.delete(scope, &credential.access_secret_id).await;
            if let Some(secret_id) = &credential.refresh_secret_id {
                let _ = self.store.delete(scope, secret_id).await;
            }
            return Err(error);
        }
        self.state
            .lock()
            .expect("credential vault state lock poisoned")
            .credentials
            .insert(
                (scope.clone(), credential.credential_id.clone()),
                credential,
            );
        Ok(())
    }

    pub async fn remove_connector_account(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        account_id: &str,
    ) -> anyhow::Result<Option<(String, u64)>> {
        scope.validate()?;
        if let Some(metadata) = &self.metadata {
            let removed = metadata
                .delete_account(scope, connector_id, account_id)
                .await?;
            let Some(account) = removed else {
                return Ok(None);
            };
            self.state
                .lock()
                .expect("credential vault state lock poisoned")
                .accounts
                .remove(&(
                    scope.clone(),
                    connector_id.to_string(),
                    account_id.to_string(),
                ));
            let remaining = metadata
                .count_credential_bindings(scope, &account.credential_id)
                .await?;
            Ok(Some((account.credential_id, remaining)))
        } else {
            let mut state = self
                .state
                .lock()
                .expect("credential vault state lock poisoned");
            let removed = state.accounts.remove(&(
                scope.clone(),
                connector_id.to_string(),
                account_id.to_string(),
            ));
            let Some(account) = removed else {
                return Ok(None);
            };
            let remaining = state
                .accounts
                .values()
                .filter(|candidate| {
                    candidate.scope == *scope && candidate.credential_id == account.credential_id
                })
                .count() as u64;
            Ok(Some((account.credential_id, remaining)))
        }
    }

    pub async fn revoke_provider_credential(
        &self,
        scope: &CredentialScope,
        credential_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        scope.validate()?;
        let credential = if let Some(metadata) = &self.metadata {
            let Some(credential) = metadata
                .revoke_credential_if_unbound(scope, credential_id, now)
                .await?
            else {
                return Ok(false);
            };
            self.state
                .lock()
                .expect("credential vault state lock poisoned")
                .credentials
                .remove(&(scope.clone(), credential_id.to_string()));
            credential
        } else {
            let mut state = self
                .state
                .lock()
                .expect("credential vault state lock poisoned");
            anyhow::ensure!(
                !state.accounts.values().any(|account| {
                    account.scope == *scope && account.credential_id == credential_id
                }),
                "provider credential is still bound to a connector"
            );
            let Some(credential) = state
                .credentials
                .get(&(scope.clone(), credential_id.to_string()))
                .cloned()
            else {
                return Ok(false);
            };
            if credential.revoked_at.is_some() {
                return Ok(false);
            }
            state
                .credentials
                .remove(&(scope.clone(), credential_id.to_string()));
            credential
        };
        self.store
            .delete(scope, &credential.access_secret_id)
            .await?;
        if let Some(secret_id) = &credential.refresh_secret_id {
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
        let (account, cached_credential) = {
            let state = self
                .state
                .lock()
                .expect("credential vault state lock poisoned");
            let account = state
                .accounts
                .get(&(
                    scope.clone(),
                    connector_id.to_string(),
                    account_id.to_string(),
                ))
                .cloned();
            let credential = account.as_ref().and_then(|account| {
                state
                    .credentials
                    .get(&(scope.clone(), account.credential_id.clone()))
                    .cloned()
            });
            (account, credential)
        };
        let account = match account {
            Some(account) => account,
            None => self
                .metadata
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("connector account is unavailable"))?
                .get_account(scope, connector_id, account_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("connector account is unavailable"))?,
        };
        anyhow::ensure!(
            required_scopes.is_subset(&account.allowed_scopes),
            "connector account lacks required scopes"
        );
        let credential = match cached_credential {
            Some(credential) => credential,
            None => self
                .get_provider_credential(scope, &account.credential_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("provider credential is unavailable"))?,
        };
        anyhow::ensure!(
            required_scopes.is_subset(&credential.granted_scopes),
            "provider credential lacks required scopes"
        );
        anyhow::ensure!(
            credential.revoked_at.is_none(),
            "provider credential is revoked"
        );
        anyhow::ensure!(
            credential
                .expires_at
                .is_none_or(|expiry| expiry > Utc::now()),
            "provider credential authorization expired"
        );
        self.store
            .load(scope, &credential.access_secret_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connector credential is unavailable"))
    }

    pub async fn get_provider_credential(
        &self,
        scope: &CredentialScope,
        credential_id: &str,
    ) -> anyhow::Result<Option<ProviderCredential>> {
        if let Some(credential) = self
            .state
            .lock()
            .expect("credential vault state lock poisoned")
            .credentials
            .get(&(scope.clone(), credential_id.to_string()))
            .cloned()
        {
            return Ok(Some(credential));
        }
        match &self.metadata {
            Some(metadata) => metadata.get_credential(scope, credential_id).await,
            None => Ok(None),
        }
    }
}

fn validate_connector_account(account: &ConnectorAccount) -> anyhow::Result<()> {
    account.scope.validate()?;
    for value in [
        &account.account_id,
        &account.connector_id,
        &account.credential_id,
    ] {
        anyhow::ensure!(
            !value.trim().is_empty(),
            "connector account field is required"
        );
        anyhow::ensure!(value.len() <= 255, "connector account field is too long");
    }
    Ok(())
}

fn validate_provider_credential(credential: &ProviderCredential) -> anyhow::Result<()> {
    for value in [
        &credential.credential_id,
        &credential.provider_id,
        &credential.provider_subject,
    ] {
        anyhow::ensure!(
            !value.trim().is_empty(),
            "provider credential field is required"
        );
        anyhow::ensure!(value.len() <= 255, "provider credential field is too long");
    }
    anyhow::ensure!(
        credential.refresh_secret_id.as_ref() != Some(&credential.access_secret_id),
        "access and refresh secret IDs must differ"
    );
    Ok(())
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
            .register_provider_credential(
                &account_scope,
                ProviderCredential {
                    access_secret_id: id,
                    credential_id: "fake-principal".into(),
                    expires_at: None,
                    granted_scopes: BTreeSet::from(["contacts.read".into(), "mail.read".into()]),
                    provider_id: "fake".into(),
                    provider_subject: "provider-user".into(),
                    refresh_secret_id: None,
                    revoked_at: None,
                },
            )
            .unwrap();
        vault
            .register_account(ConnectorAccount {
                account_id: "primary".into(),
                allowed_scopes: BTreeSet::from(["mail.read".into()]),
                connector_id: "mail.fake".into(),
                credential_id: "fake-principal".into(),
                scope: account_scope.clone(),
            })
            .unwrap();
        vault
            .register_account(ConnectorAccount {
                account_id: "primary".into(),
                allowed_scopes: BTreeSet::from(["contacts.read".into()]),
                connector_id: "contacts.fake".into(),
                credential_id: "fake-principal".into(),
                scope: account_scope.clone(),
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
                .lease_for_connector(
                    &account_scope,
                    "contacts.fake",
                    "primary",
                    &BTreeSet::from(["contacts.read".into()])
                )
                .await
                .is_ok()
        );
        assert!(
            vault
                .lease_for_connector(&account_scope, "calendar.fake", "primary", &BTreeSet::new())
                .await
                .is_err()
        );
    }

    #[test]
    fn serialized_metadata_never_contains_secret_material() {
        let account = ConnectorAccount {
            account_id: "primary".into(),
            allowed_scopes: BTreeSet::new(),
            connector_id: "mail.fake".into(),
            credential_id: "fake-principal".into(),
            scope: scope("com.example.mail"),
        };
        let json = serde_json::to_string(&account).unwrap();
        assert!(json.contains("fake-principal"));
        assert!(!json.contains("mail.account.primary"));
        assert!(!json.contains("credential-value"));
    }
}
