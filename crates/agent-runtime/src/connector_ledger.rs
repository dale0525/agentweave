use crate::credential::CredentialScope;
use crate::storage::Storage;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorLedgerEntry {
    pub action_hash: String,
    pub output: Value,
}

#[async_trait]
pub trait ConnectorActionLedger: Send + Sync {
    async fn get(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<ConnectorLedgerEntry>>;

    async fn record(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
        entry: ConnectorLedgerEntry,
    ) -> anyhow::Result<ConnectorLedgerEntry>;
}

#[derive(Default)]
pub struct InMemoryConnectorActionLedger {
    entries: Mutex<BTreeMap<(CredentialScope, String, String), ConnectorLedgerEntry>>,
}

#[async_trait]
impl ConnectorActionLedger for InMemoryConnectorActionLedger {
    async fn get(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<ConnectorLedgerEntry>> {
        validate_key(scope, connector_id, idempotency_key)?;
        Ok(self
            .entries
            .lock()
            .expect("connector ledger lock poisoned")
            .get(&(
                scope.clone(),
                connector_id.to_string(),
                idempotency_key.to_string(),
            ))
            .cloned())
    }

    async fn record(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
        entry: ConnectorLedgerEntry,
    ) -> anyhow::Result<ConnectorLedgerEntry> {
        validate_key(scope, connector_id, idempotency_key)?;
        validate_entry(&entry)?;
        let key = (
            scope.clone(),
            connector_id.to_string(),
            idempotency_key.to_string(),
        );
        let mut entries = self.entries.lock().expect("connector ledger lock poisoned");
        if let Some(existing) = entries.get(&key) {
            anyhow::ensure!(
                existing.action_hash == entry.action_hash,
                "idempotency key argument conflict"
            );
            return Ok(existing.clone());
        }
        entries.insert(key, entry.clone());
        Ok(entry)
    }
}

#[derive(Clone)]
pub struct SqliteConnectorActionLedger {
    pool: SqlitePool,
}

impl SqliteConnectorActionLedger {
    pub async fn from_storage(storage: &Storage) -> anyhow::Result<Self> {
        let ledger = Self {
            pool: storage.pool().clone(),
        };
        ledger.migrate().await?;
        Ok(ledger)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS connector_action_ledger (
                app_id TEXT NOT NULL,
                tenant_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                connector_id TEXT NOT NULL,
                idempotency_key TEXT NOT NULL,
                action_hash TEXT NOT NULL,
                output_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY(app_id, tenant_id, user_id, connector_id, idempotency_key)
            )"#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl ConnectorActionLedger for SqliteConnectorActionLedger {
    async fn get(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<ConnectorLedgerEntry>> {
        validate_key(scope, connector_id, idempotency_key)?;
        let row = sqlx::query(
            "SELECT action_hash, output_json FROM connector_action_ledger WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND connector_id = ? AND idempotency_key = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(connector_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(entry_from_row).transpose()
    }

    async fn record(
        &self,
        scope: &CredentialScope,
        connector_id: &str,
        idempotency_key: &str,
        entry: ConnectorLedgerEntry,
    ) -> anyhow::Result<ConnectorLedgerEntry> {
        validate_key(scope, connector_id, idempotency_key)?;
        validate_entry(&entry)?;
        sqlx::query(
            "INSERT OR IGNORE INTO connector_action_ledger(app_id, tenant_id, user_id, connector_id, idempotency_key, action_hash, output_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(connector_id)
        .bind(idempotency_key)
        .bind(&entry.action_hash)
        .bind(serde_json::to_string(&entry.output)?)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        let stored = self
            .get(scope, connector_id, idempotency_key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("connector ledger entry was not persisted"))?;
        anyhow::ensure!(
            stored.action_hash == entry.action_hash,
            "idempotency key argument conflict"
        );
        Ok(stored)
    }
}

fn validate_key(
    scope: &CredentialScope,
    connector_id: &str,
    idempotency_key: &str,
) -> anyhow::Result<()> {
    scope.validate()?;
    anyhow::ensure!(!connector_id.trim().is_empty(), "connector id is required");
    anyhow::ensure!(
        !idempotency_key.trim().is_empty(),
        "connector idempotency key is required"
    );
    anyhow::ensure!(
        connector_id.len() <= 255 && idempotency_key.len() <= 512,
        "connector ledger key exceeds limit"
    );
    Ok(())
}

fn validate_entry(entry: &ConnectorLedgerEntry) -> anyhow::Result<()> {
    anyhow::ensure!(
        entry.action_hash.len() == 64
            && entry
                .action_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "connector action hash is invalid"
    );
    anyhow::ensure!(
        serde_json::to_vec(&entry.output)?.len() <= 1024 * 1024,
        "connector ledger output exceeds limit"
    );
    Ok(())
}

fn entry_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<ConnectorLedgerEntry> {
    Ok(ConnectorLedgerEntry {
        action_hash: row.try_get("action_hash")?,
        output: serde_json::from_str(row.try_get("output_json")?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(app_id: &str) -> CredentialScope {
        CredentialScope {
            app_id: app_id.into(),
            tenant_id: "tenant".into(),
            user_id: "user".into(),
        }
    }

    #[tokio::test]
    async fn sqlite_ledger_survives_reconstruction_and_is_scope_isolated() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let first = SqliteConnectorActionLedger::from_storage(&storage)
            .await
            .unwrap();
        let entry = ConnectorLedgerEntry {
            action_hash: "a".repeat(64),
            output: json!({"delivery": "sent"}),
        };
        first
            .record(&scope("app-a"), "mail", "send-1", entry.clone())
            .await
            .unwrap();

        let resumed = SqliteConnectorActionLedger::from_storage(&storage)
            .await
            .unwrap();
        assert_eq!(
            resumed
                .get(&scope("app-a"), "mail", "send-1")
                .await
                .unwrap(),
            Some(entry)
        );
        assert!(
            resumed
                .get(&scope("app-b"), "mail", "send-1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            resumed
                .record(
                    &scope("app-a"),
                    "mail",
                    "send-1",
                    ConnectorLedgerEntry {
                        action_hash: "b".repeat(64),
                        output: json!({}),
                    },
                )
                .await
                .is_err()
        );
    }
}
