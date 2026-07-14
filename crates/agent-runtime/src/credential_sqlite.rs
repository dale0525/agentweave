use crate::credential::{
    ConnectorAccount, CredentialScope, OAuthAuthorizationState, OAuthTokenRecord, SecretId,
};
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteCredentialMetadataStore {
    pool: SqlitePool,
}

impl SqliteCredentialMetadataStore {
    pub async fn from_storage(storage: &Storage) -> anyhow::Result<Self> {
        let store = Self {
            pool: storage.pool().clone(),
        };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        for statement in schema_statements() {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_account(&self, account: &ConnectorAccount) -> anyhow::Result<()> {
        validate_account(account)?;
        sqlx::query(
            r#"INSERT INTO connector_accounts(app_id, tenant_id, user_id, account_id, connector_id, provider_id, secret_id, granted_scopes_json, expires_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(app_id, tenant_id, user_id, account_id) DO UPDATE SET connector_id = excluded.connector_id, provider_id = excluded.provider_id, secret_id = excluded.secret_id, granted_scopes_json = excluded.granted_scopes_json, expires_at = excluded.expires_at, updated_at = excluded.updated_at"#,
        )
        .bind(&account.scope.app_id)
        .bind(&account.scope.tenant_id)
        .bind(&account.scope.user_id)
        .bind(&account.account_id)
        .bind(&account.connector_id)
        .bind(&account.provider_id)
        .bind(account.secret_id.as_str())
        .bind(serde_json::to_string(&account.granted_scopes)?)
        .bind(account.expires_at.map(|value| value.to_rfc3339()))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_account(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<Option<ConnectorAccount>> {
        scope.validate()?;
        let row = sqlx::query(
            "SELECT account_id, connector_id, provider_id, secret_id, granted_scopes_json, expires_at FROM connector_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| account_from_row(scope, row)).transpose()
    }

    pub async fn list_accounts(
        &self,
        scope: &CredentialScope,
    ) -> anyhow::Result<Vec<ConnectorAccount>> {
        scope.validate()?;
        let rows = sqlx::query(
            "SELECT account_id, connector_id, provider_id, secret_id, granted_scopes_json, expires_at FROM connector_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? ORDER BY connector_id, account_id",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| account_from_row(scope, row))
            .collect()
    }

    pub async fn delete_account(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<bool> {
        scope.validate()?;
        let result = sqlx::query(
            "DELETE FROM connector_accounts WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn save_oauth_state(
        &self,
        scope: &CredentialScope,
        state: &OAuthAuthorizationState,
    ) -> anyhow::Result<()> {
        validate_oauth_state(scope, state)?;
        sqlx::query(
            "INSERT INTO oauth_authorization_states(app_id, tenant_id, user_id, state_id, connector_id, account_id, pkce_secret_id, redirect_uri, requested_scopes_json, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(&state.state_id)
        .bind(&state.connector_id)
        .bind(&state.account_id)
        .bind(state.pkce_verifier_secret_id.as_str())
        .bind(&state.redirect_uri)
        .bind(serde_json::to_string(&state.requested_scopes)?)
        .bind(state.expires_at.to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn consume_oauth_state(
        &self,
        scope: &CredentialScope,
        state_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<OAuthAuthorizationState> {
        scope.validate()?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT state_id, connector_id, account_id, pkce_secret_id, redirect_uri, requested_scopes_json, expires_at FROM oauth_authorization_states WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND state_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(state_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("OAuth authorization state is unavailable"))?;
        let state = oauth_state_from_row(row)?;
        anyhow::ensure!(state.expires_at > now, "OAuth authorization state expired");
        let deleted = sqlx::query(
            "DELETE FROM oauth_authorization_states WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND state_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(state_id)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            deleted.rows_affected() == 1,
            "OAuth state was consumed elsewhere"
        );
        tx.commit().await?;
        Ok(state)
    }

    pub async fn upsert_oauth_tokens(
        &self,
        scope: &CredentialScope,
        record: &OAuthTokenRecord,
    ) -> anyhow::Result<()> {
        validate_token_record(scope, record)?;
        sqlx::query(
            r#"INSERT INTO oauth_token_records(app_id, tenant_id, user_id, account_id, connector_id, provider_id, access_secret_id, refresh_secret_id, granted_scopes_json, expires_at, revoked_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(app_id, tenant_id, user_id, account_id) DO UPDATE SET connector_id = excluded.connector_id, provider_id = excluded.provider_id, access_secret_id = excluded.access_secret_id, refresh_secret_id = excluded.refresh_secret_id, granted_scopes_json = excluded.granted_scopes_json, expires_at = excluded.expires_at, revoked_at = excluded.revoked_at, updated_at = excluded.updated_at"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(&record.account_id)
        .bind(&record.connector_id)
        .bind(&record.provider_id)
        .bind(record.access_token_secret_id.as_str())
        .bind(record.refresh_token_secret_id.as_ref().map(SecretId::as_str))
        .bind(serde_json::to_string(&record.granted_scopes)?)
        .bind(record.expires_at.map(|value| value.to_rfc3339()))
        .bind(record.revoked_at.map(|value| value.to_rfc3339()))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_oauth_tokens(
        &self,
        scope: &CredentialScope,
        account_id: &str,
    ) -> anyhow::Result<Option<OAuthTokenRecord>> {
        scope.validate()?;
        let row = sqlx::query(
            "SELECT account_id, connector_id, provider_id, access_secret_id, refresh_secret_id, granted_scopes_json, expires_at, revoked_at FROM oauth_token_records WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(token_record_from_row).transpose()
    }

    pub async fn revoke_oauth_tokens(
        &self,
        scope: &CredentialScope,
        account_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        scope.validate()?;
        let result = sqlx::query(
            "UPDATE oauth_token_records SET revoked_at = ?, updated_at = ? WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND account_id = ? AND revoked_at IS NULL",
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(account_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

fn schema_statements() -> [&'static str; 3] {
    [
        r#"CREATE TABLE IF NOT EXISTS connector_accounts (app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, account_id TEXT NOT NULL, connector_id TEXT NOT NULL, provider_id TEXT NOT NULL, secret_id TEXT NOT NULL, granted_scopes_json TEXT NOT NULL, expires_at TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, tenant_id, user_id, account_id))"#,
        r#"CREATE TABLE IF NOT EXISTS oauth_authorization_states (app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, state_id TEXT NOT NULL, connector_id TEXT NOT NULL, account_id TEXT NOT NULL, pkce_secret_id TEXT NOT NULL, redirect_uri TEXT NOT NULL, requested_scopes_json TEXT NOT NULL, expires_at TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY(app_id, tenant_id, user_id, state_id))"#,
        r#"CREATE TABLE IF NOT EXISTS oauth_token_records (app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, account_id TEXT NOT NULL, connector_id TEXT NOT NULL, provider_id TEXT NOT NULL, access_secret_id TEXT NOT NULL, refresh_secret_id TEXT, granted_scopes_json TEXT NOT NULL, expires_at TEXT, revoked_at TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(app_id, tenant_id, user_id, account_id))"#,
    ]
}

fn validate_account(account: &ConnectorAccount) -> anyhow::Result<()> {
    account.scope.validate()?;
    for value in [
        &account.account_id,
        &account.connector_id,
        &account.provider_id,
    ] {
        anyhow::ensure!(
            !value.trim().is_empty(),
            "credential account field is required"
        );
        anyhow::ensure!(value.len() <= 255, "credential account field is too long");
    }
    Ok(())
}

fn validate_oauth_state(
    scope: &CredentialScope,
    state: &OAuthAuthorizationState,
) -> anyhow::Result<()> {
    scope.validate()?;
    for value in [
        &state.state_id,
        &state.connector_id,
        &state.account_id,
        &state.redirect_uri,
    ] {
        anyhow::ensure!(!value.trim().is_empty(), "OAuth state field is required");
        anyhow::ensure!(value.len() <= 2048, "OAuth state field is too long");
    }
    Ok(())
}

fn validate_token_record(scope: &CredentialScope, record: &OAuthTokenRecord) -> anyhow::Result<()> {
    scope.validate()?;
    for value in [
        &record.account_id,
        &record.connector_id,
        &record.provider_id,
    ] {
        anyhow::ensure!(!value.trim().is_empty(), "OAuth token field is required");
        anyhow::ensure!(value.len() <= 255, "OAuth token field is too long");
    }
    Ok(())
}

fn account_from_row(
    scope: &CredentialScope,
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<ConnectorAccount> {
    Ok(ConnectorAccount {
        account_id: row.try_get("account_id")?,
        connector_id: row.try_get("connector_id")?,
        provider_id: row.try_get("provider_id")?,
        secret_id: SecretId::parse(row.try_get("secret_id")?)?,
        scope: scope.clone(),
        granted_scopes: serde_json::from_str(row.try_get("granted_scopes_json")?)?,
        expires_at: parse_time(row.try_get("expires_at")?)?,
    })
}

fn oauth_state_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<OAuthAuthorizationState> {
    Ok(OAuthAuthorizationState {
        state_id: row.try_get("state_id")?,
        connector_id: row.try_get("connector_id")?,
        account_id: row.try_get("account_id")?,
        pkce_verifier_secret_id: SecretId::parse(row.try_get("pkce_secret_id")?)?,
        redirect_uri: row.try_get("redirect_uri")?,
        requested_scopes: serde_json::from_str(row.try_get("requested_scopes_json")?)?,
        expires_at: parse_required_time(row.try_get("expires_at")?)?,
    })
}

fn token_record_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<OAuthTokenRecord> {
    Ok(OAuthTokenRecord {
        account_id: row.try_get("account_id")?,
        connector_id: row.try_get("connector_id")?,
        provider_id: row.try_get("provider_id")?,
        access_token_secret_id: SecretId::parse(row.try_get("access_secret_id")?)?,
        refresh_token_secret_id: row
            .try_get::<Option<String>, _>("refresh_secret_id")?
            .map(|value| SecretId::parse(&value))
            .transpose()?,
        granted_scopes: serde_json::from_str(row.try_get("granted_scopes_json")?)?,
        expires_at: parse_time(row.try_get("expires_at")?)?,
        revoked_at: parse_time(row.try_get("revoked_at")?)?,
    })
}

fn parse_time(value: Option<String>) -> anyhow::Result<Option<DateTime<Utc>>> {
    value.map(|value| parse_required_time(&value)).transpose()
}

fn parse_required_time(value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{CredentialVault, InMemorySecretStore, SecretMaterial, SecretStore};
    use chrono::Duration;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn scope(app_id: &str) -> CredentialScope {
        CredentialScope {
            app_id: app_id.into(),
            tenant_id: "tenant".into(),
            user_id: "user".into(),
        }
    }

    #[tokio::test]
    async fn account_and_oauth_metadata_survive_vault_reconstruction() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
            .await
            .unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let vault = CredentialVault::new_persistent(secrets.clone(), metadata.clone());
        let account_scope = scope("app-a");
        let secret_id = SecretId::parse("mail.primary.access").unwrap();
        secrets
            .save(
                &account_scope,
                &secret_id,
                SecretMaterial::new("access-token").unwrap(),
            )
            .await
            .unwrap();
        vault
            .register_account_persistent(ConnectorAccount {
                account_id: "primary".into(),
                connector_id: "mail".into(),
                provider_id: "example".into(),
                secret_id: secret_id.clone(),
                scope: account_scope.clone(),
                granted_scopes: BTreeSet::from(["mail.read".into()]),
                expires_at: Some(Utc::now() + Duration::minutes(10)),
            })
            .await
            .unwrap();

        let resumed = CredentialVault::new_persistent(secrets, metadata);
        let leased = resumed
            .lease_for_connector(
                &account_scope,
                "mail",
                "primary",
                &BTreeSet::from(["mail.read".into()]),
            )
            .await
            .unwrap();
        assert_eq!(leased.expose_bytes(), b"access-token");
        assert!(
            resumed
                .lease_for_connector(&scope("app-b"), "mail", "primary", &BTreeSet::new(),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn oauth_state_is_single_use_and_pkce_secret_is_scrubbed() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
            .await
            .unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let vault = CredentialVault::new_persistent(secrets.clone(), metadata);
        let account_scope = scope("app-a");
        let verifier_id = SecretId::parse("oauth.state.verifier").unwrap();
        let state = OAuthAuthorizationState {
            state_id: "state-1".into(),
            connector_id: "mail".into(),
            account_id: "primary".into(),
            pkce_verifier_secret_id: verifier_id.clone(),
            redirect_uri: "http://127.0.0.1/callback".into(),
            requested_scopes: BTreeSet::from(["mail.read".into()]),
            expires_at: Utc::now() + Duration::minutes(5),
        };
        vault
            .begin_oauth_authorization(
                &account_scope,
                state,
                SecretMaterial::new("verifier").unwrap(),
            )
            .await
            .unwrap();
        let (_, verifier) = vault
            .consume_oauth_authorization(&account_scope, "state-1", Utc::now())
            .await
            .unwrap();
        assert_eq!(verifier.expose_bytes(), b"verifier");
        assert!(
            secrets
                .load(&account_scope, &verifier_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            vault
                .consume_oauth_authorization(&account_scope, "state-1", Utc::now())
                .await
                .is_err()
        );
    }
}
