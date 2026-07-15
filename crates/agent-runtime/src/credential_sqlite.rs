use crate::credential::{
    ConnectorAccount, CredentialScope, OAuthAuthorizationState, ProviderCredential, SecretId,
};
use crate::storage::Storage;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::collections::BTreeSet;

const CREATE_CREDENTIAL_RECORDS: &str = r#"CREATE TABLE IF NOT EXISTS credential_records (
    app_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    credential_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    access_secret_id TEXT NOT NULL,
    refresh_secret_id TEXT,
    granted_scopes_json TEXT NOT NULL,
    expires_at TEXT,
    revoked_at TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(app_id, tenant_id, user_id, credential_id)
)"#;

const CREATE_CONNECTOR_ACCOUNTS: &str = r#"CREATE TABLE IF NOT EXISTS connector_accounts (
    app_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    credential_id TEXT NOT NULL,
    allowed_scopes_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(app_id, tenant_id, user_id, connector_id, account_id),
    FOREIGN KEY(app_id, tenant_id, user_id, credential_id)
        REFERENCES credential_records(app_id, tenant_id, user_id, credential_id)
        ON DELETE RESTRICT
)"#;

const CREATE_OAUTH_STATES: &str = r#"CREATE TABLE IF NOT EXISTS oauth_authorization_states (
    app_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    state_id TEXT NOT NULL,
    connector_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    pkce_secret_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    requested_scopes_json TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(app_id, tenant_id, user_id, state_id)
)"#;

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
        sqlx::query(CREATE_CREDENTIAL_RECORDS)
            .execute(&mut *tx)
            .await?;
        sqlx::query(CREATE_OAUTH_STATES).execute(&mut *tx).await?;

        let connector_columns = table_columns(&mut tx, "connector_accounts").await?;
        if !connector_columns.is_empty() && !connector_columns.contains("credential_id") {
            sqlx::query(
                r#"INSERT OR IGNORE INTO credential_records(
                    app_id, tenant_id, user_id, credential_id, provider_id, provider_subject,
                    access_secret_id, refresh_secret_id, granted_scopes_json, expires_at,
                    revoked_at, updated_at
                )
                SELECT app_id, tenant_id, user_id, secret_id, provider_id, account_id,
                    secret_id, NULL, granted_scopes_json, expires_at, NULL, updated_at
                FROM connector_accounts"#,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("ALTER TABLE connector_accounts RENAME TO connector_accounts_legacy")
                .execute(&mut *tx)
                .await?;
            sqlx::query(CREATE_CONNECTOR_ACCOUNTS)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                r#"INSERT INTO connector_accounts(
                    app_id, tenant_id, user_id, connector_id, account_id, credential_id,
                    allowed_scopes_json, updated_at
                )
                SELECT app_id, tenant_id, user_id, connector_id, account_id, secret_id,
                    granted_scopes_json, updated_at
                FROM connector_accounts_legacy"#,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DROP TABLE connector_accounts_legacy")
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query(CREATE_CONNECTOR_ACCOUNTS)
                .execute(&mut *tx)
                .await?;
        }

        if !table_columns(&mut tx, "oauth_token_records")
            .await?
            .is_empty()
        {
            sqlx::query(
                r#"INSERT OR IGNORE INTO credential_records(
                    app_id, tenant_id, user_id, credential_id, provider_id, provider_subject,
                    access_secret_id, refresh_secret_id, granted_scopes_json, expires_at,
                    revoked_at, updated_at
                )
                SELECT app_id, tenant_id, user_id, access_secret_id, provider_id, account_id,
                    access_secret_id, refresh_secret_id, granted_scopes_json, expires_at,
                    revoked_at, updated_at
                FROM oauth_token_records"#,
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"INSERT OR IGNORE INTO connector_accounts(
                    app_id, tenant_id, user_id, connector_id, account_id, credential_id,
                    allowed_scopes_json, updated_at
                )
                SELECT app_id, tenant_id, user_id, connector_id, account_id, access_secret_id,
                    granted_scopes_json, updated_at
                FROM oauth_token_records"#,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_credential(
        &self,
        scope: &CredentialScope,
        credential: &ProviderCredential,
    ) -> anyhow::Result<()> {
        validate_credential(scope, credential)?;
        sqlx::query(
            r#"INSERT INTO credential_records(
                app_id, tenant_id, user_id, credential_id, provider_id, provider_subject,
                access_secret_id, refresh_secret_id, granted_scopes_json, expires_at,
                revoked_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(app_id, tenant_id, user_id, credential_id) DO UPDATE SET
                provider_id = excluded.provider_id,
                provider_subject = excluded.provider_subject,
                access_secret_id = excluded.access_secret_id,
                refresh_secret_id = excluded.refresh_secret_id,
                granted_scopes_json = excluded.granted_scopes_json,
                expires_at = excluded.expires_at,
                revoked_at = excluded.revoked_at,
                updated_at = excluded.updated_at"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(&credential.credential_id)
        .bind(&credential.provider_id)
        .bind(&credential.provider_subject)
        .bind(credential.access_secret_id.as_str())
        .bind(credential.refresh_secret_id.as_ref().map(SecretId::as_str))
        .bind(serde_json::to_string(&credential.granted_scopes)?)
        .bind(credential.expires_at.map(|value| value.to_rfc3339()))
        .bind(credential.revoked_at.map(|value| value.to_rfc3339()))
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_credential(
        &self,
        scope: &CredentialScope,
        credential_id: &str,
    ) -> anyhow::Result<Option<ProviderCredential>> {
        scope.validate()?;
        let row = sqlx::query(
            r#"SELECT credential_id, provider_id, provider_subject, access_secret_id,
                refresh_secret_id, granted_scopes_json, expires_at, revoked_at
            FROM credential_records
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(credential_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(credential_from_row).transpose()
    }

    pub async fn revoke_credential_if_unbound(
        &self,
        scope: &CredentialScope,
        credential_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<ProviderCredential>> {
        scope.validate()?;
        let mut tx = self.pool.begin().await?;
        let credential = sqlx::query(
            r#"SELECT credential_id, provider_id, provider_subject, access_secret_id,
                refresh_secret_id, granted_scopes_json, expires_at, revoked_at
            FROM credential_records
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(credential_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(credential_from_row)
        .transpose()?;
        let Some(credential) = credential else {
            return Ok(None);
        };
        if credential.revoked_at.is_some() {
            return Ok(None);
        }
        let bindings: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM connector_accounts
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(credential_id)
        .fetch_one(&mut *tx)
        .await?;
        anyhow::ensure!(
            bindings == 0,
            "provider credential is still bound to a connector"
        );
        let result = sqlx::query(
            r#"UPDATE credential_records SET revoked_at = ?, updated_at = ?
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?
                AND revoked_at IS NULL"#,
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(credential_id)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "provider credential changed concurrently"
        );
        tx.commit().await?;
        Ok(Some(credential))
    }

    pub async fn upsert_account(&self, account: &ConnectorAccount) -> anyhow::Result<()> {
        validate_account(account)?;
        let mut tx = self.pool.begin().await?;
        let credential = sqlx::query(
            r#"SELECT credential_id, provider_id, provider_subject, access_secret_id,
                refresh_secret_id, granted_scopes_json, expires_at, revoked_at
            FROM credential_records
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?"#,
        )
        .bind(&account.scope.app_id)
        .bind(&account.scope.tenant_id)
        .bind(&account.scope.user_id)
        .bind(&account.credential_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(credential_from_row)
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("connector account credential is unavailable"))?;
        anyhow::ensure!(
            account.allowed_scopes.is_subset(&credential.granted_scopes),
            "connector account scopes exceed provider credential grant"
        );
        anyhow::ensure!(
            credential.revoked_at.is_none(),
            "connector account credential is revoked"
        );
        sqlx::query(
            r#"INSERT INTO connector_accounts(
                app_id, tenant_id, user_id, connector_id, account_id, credential_id,
                allowed_scopes_json, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(app_id, tenant_id, user_id, connector_id, account_id) DO UPDATE SET
                credential_id = excluded.credential_id,
                allowed_scopes_json = excluded.allowed_scopes_json,
                updated_at = excluded.updated_at"#,
        )
        .bind(&account.scope.app_id)
        .bind(&account.scope.tenant_id)
        .bind(&account.scope.user_id)
        .bind(&account.connector_id)
        .bind(&account.account_id)
        .bind(&account.credential_id)
        .bind(serde_json::to_string(&account.allowed_scopes)?)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_account(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        account_id: &str,
    ) -> anyhow::Result<Option<ConnectorAccount>> {
        scope.validate()?;
        let row = sqlx::query(
            r#"SELECT connector_id, account_id, credential_id, allowed_scopes_json
            FROM connector_accounts
            WHERE app_id = ? AND tenant_id = ? AND user_id = ?
                AND connector_id = ? AND account_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(connector_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| account_from_row(scope, row)).transpose()
    }

    pub async fn list_accounts(
        &self,
        scope: &CredentialScope,
        connector_id: Option<&str>,
    ) -> anyhow::Result<Vec<ConnectorAccount>> {
        scope.validate()?;
        let rows = match connector_id {
            Some(connector_id) => {
                sqlx::query(
                    r#"SELECT connector_id, account_id, credential_id, allowed_scopes_json
                    FROM connector_accounts
                    WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND connector_id = ?
                    ORDER BY account_id"#,
                )
                .bind(&scope.app_id)
                .bind(&scope.tenant_id)
                .bind(&scope.user_id)
                .bind(connector_id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    r#"SELECT connector_id, account_id, credential_id, allowed_scopes_json
                    FROM connector_accounts
                    WHERE app_id = ? AND tenant_id = ? AND user_id = ?
                    ORDER BY connector_id, account_id"#,
                )
                .bind(&scope.app_id)
                .bind(&scope.tenant_id)
                .bind(&scope.user_id)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.into_iter()
            .map(|row| account_from_row(scope, row))
            .collect()
    }

    pub async fn delete_account(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        account_id: &str,
    ) -> anyhow::Result<Option<ConnectorAccount>> {
        scope.validate()?;
        let mut tx = self.pool.begin().await?;
        let account = sqlx::query(
            r#"SELECT connector_id, account_id, credential_id, allowed_scopes_json
            FROM connector_accounts
            WHERE app_id = ? AND tenant_id = ? AND user_id = ?
                AND connector_id = ? AND account_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(connector_id)
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(|row| account_from_row(scope, row))
        .transpose()?;
        let Some(account) = account else {
            return Ok(None);
        };
        let result = sqlx::query(
            r#"DELETE FROM connector_accounts
            WHERE app_id = ? AND tenant_id = ? AND user_id = ?
                AND connector_id = ? AND account_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(connector_id)
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "connector account changed concurrently"
        );
        tx.commit().await?;
        Ok(Some(account))
    }

    pub async fn count_credential_bindings(
        &self,
        scope: &CredentialScope,
        credential_id: &str,
    ) -> anyhow::Result<u64> {
        scope.validate()?;
        let count: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*) FROM connector_accounts
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND credential_id = ?"#,
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(credential_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(u64::try_from(count)?)
    }

    pub async fn save_oauth_state(
        &self,
        scope: &CredentialScope,
        state: &OAuthAuthorizationState,
    ) -> anyhow::Result<()> {
        validate_oauth_state(scope, state)?;
        sqlx::query(
            r#"INSERT INTO oauth_authorization_states(
                app_id, tenant_id, user_id, state_id, connector_id, account_id,
                pkce_secret_id, redirect_uri, requested_scopes_json, expires_at, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
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
            r#"SELECT state_id, connector_id, account_id, pkce_secret_id, redirect_uri,
                requested_scopes_json, expires_at
            FROM oauth_authorization_states
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND state_id = ?"#,
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
            r#"DELETE FROM oauth_authorization_states
            WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND state_id = ?"#,
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
}

async fn table_columns(
    connection: &mut SqliteConnection,
    table: &str,
) -> anyhow::Result<BTreeSet<String>> {
    let rows = sqlx::query("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(connection)
        .await?;
    rows.into_iter()
        .map(|row| row.try_get("name").map_err(Into::into))
        .collect()
}

fn validate_account(account: &ConnectorAccount) -> anyhow::Result<()> {
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

fn validate_credential(
    scope: &CredentialScope,
    credential: &ProviderCredential,
) -> anyhow::Result<()> {
    scope.validate()?;
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

fn account_from_row(
    scope: &CredentialScope,
    row: sqlx::sqlite::SqliteRow,
) -> anyhow::Result<ConnectorAccount> {
    Ok(ConnectorAccount {
        account_id: row.try_get("account_id")?,
        allowed_scopes: serde_json::from_str(row.try_get("allowed_scopes_json")?)?,
        connector_id: row.try_get("connector_id")?,
        credential_id: row.try_get("credential_id")?,
        scope: scope.clone(),
    })
}

fn credential_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<ProviderCredential> {
    Ok(ProviderCredential {
        access_secret_id: SecretId::parse(row.try_get("access_secret_id")?)?,
        credential_id: row.try_get("credential_id")?,
        expires_at: parse_time(row.try_get("expires_at")?)?,
        granted_scopes: serde_json::from_str(row.try_get("granted_scopes_json")?)?,
        provider_id: row.try_get("provider_id")?,
        provider_subject: row.try_get("provider_subject")?,
        refresh_secret_id: row
            .try_get::<Option<String>, _>("refresh_secret_id")?
            .map(|value| SecretId::parse(&value))
            .transpose()?,
        revoked_at: parse_time(row.try_get("revoked_at")?)?,
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
    use std::sync::Arc;

    fn scope(app_id: &str) -> CredentialScope {
        CredentialScope {
            app_id: app_id.into(),
            tenant_id: "tenant".into(),
            user_id: "user".into(),
        }
    }

    fn credential(secret_id: &SecretId) -> ProviderCredential {
        ProviderCredential {
            access_secret_id: secret_id.clone(),
            credential_id: "workspace-principal".into(),
            expires_at: Some(Utc::now() + Duration::minutes(10)),
            granted_scopes: BTreeSet::from(["calendar.read".into(), "contacts.read".into()]),
            provider_id: "workspace".into(),
            provider_subject: "provider-user-1".into(),
            refresh_secret_id: None,
            revoked_at: None,
        }
    }

    fn binding(
        account_scope: &CredentialScope,
        connector_id: &str,
        allowed_scope: &str,
    ) -> ConnectorAccount {
        ConnectorAccount {
            account_id: "primary".into(),
            allowed_scopes: BTreeSet::from([allowed_scope.into()]),
            connector_id: connector_id.into(),
            credential_id: "workspace-principal".into(),
            scope: account_scope.clone(),
        }
    }

    #[tokio::test]
    async fn shared_principal_bindings_survive_and_revoke_only_after_last_unbind() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
            .await
            .unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let vault = CredentialVault::new_persistent(secrets.clone(), metadata.clone());
        let account_scope = scope("app-a");
        let secret_id = SecretId::parse("workspace.access").unwrap();
        secrets
            .save(
                &account_scope,
                &secret_id,
                SecretMaterial::new("access-token").unwrap(),
            )
            .await
            .unwrap();
        vault
            .register_provider_credential_persistent(&account_scope, credential(&secret_id))
            .await
            .unwrap();
        for account in [
            binding(&account_scope, "calendar", "calendar.read"),
            binding(&account_scope, "contacts", "contacts.read"),
        ] {
            vault.register_account_persistent(account).await.unwrap();
        }
        let mut overbroad = binding(&account_scope, "mail", "mail.send");
        overbroad.allowed_scopes.insert("calendar.read".into());
        assert!(vault.register_account_persistent(overbroad).await.is_err());

        let resumed = CredentialVault::new_persistent(secrets.clone(), metadata);
        assert_eq!(
            resumed
                .list_connector_accounts(&account_scope, None)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            resumed
                .list_connector_accounts(&account_scope, Some("calendar"))
                .await
                .unwrap()
                .len(),
            1
        );
        for (connector_id, required) in
            [("calendar", "calendar.read"), ("contacts", "contacts.read")]
        {
            let leased = resumed
                .lease_for_connector(
                    &account_scope,
                    connector_id,
                    "primary",
                    &BTreeSet::from([required.into()]),
                )
                .await
                .unwrap();
            assert_eq!(leased.expose_bytes(), b"access-token");
        }
        let (_, remaining) = resumed
            .remove_connector_account(&account_scope, "calendar", "primary")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining, 1);
        assert!(
            resumed
                .revoke_provider_credential(&account_scope, "workspace-principal", Utc::now())
                .await
                .is_err()
        );
        let (_, remaining) = resumed
            .remove_connector_account(&account_scope, "contacts", "primary")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining, 0);
        assert!(
            resumed
                .revoke_provider_credential(&account_scope, "workspace-principal", Utc::now())
                .await
                .unwrap()
        );
        assert!(
            secrets
                .load(&account_scope, &secret_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn legacy_account_schema_migrates_without_exposing_secret_material() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            r#"CREATE TABLE connector_accounts (
                app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL,
                account_id TEXT NOT NULL, connector_id TEXT NOT NULL, provider_id TEXT NOT NULL,
                secret_id TEXT NOT NULL, granted_scopes_json TEXT NOT NULL, expires_at TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(app_id, tenant_id, user_id, account_id)
            )"#,
        )
        .execute(storage.pool())
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO connector_accounts VALUES (
                'app-a', 'tenant', 'user', 'primary', 'mail', 'imap-smtp',
                'mail.password', '["mail.read"]', NULL, '2026-07-15T00:00:00Z'
            )"#,
        )
        .execute(storage.pool())
        .await
        .unwrap();
        let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
            .await
            .unwrap();
        let secrets = Arc::new(InMemorySecretStore::default());
        let account_scope = scope("app-a");
        let secret_id = SecretId::parse("mail.password").unwrap();
        secrets
            .save(
                &account_scope,
                &secret_id,
                SecretMaterial::new("password").unwrap(),
            )
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
        assert_eq!(leased.expose_bytes(), b"password");
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
        vault
            .begin_oauth_authorization(
                &account_scope,
                OAuthAuthorizationState {
                    state_id: "state-1".into(),
                    connector_id: "calendar".into(),
                    account_id: "primary".into(),
                    pkce_verifier_secret_id: verifier_id.clone(),
                    redirect_uri: "http://127.0.0.1/callback".into(),
                    requested_scopes: BTreeSet::from(["calendar.read".into()]),
                    expires_at: Utc::now() + Duration::minutes(5),
                },
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
