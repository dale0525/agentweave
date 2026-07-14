use crate::memory::{MemoryError, MemoryResult};
use sqlx::SqlitePool;

pub const MEMORY_SQLITE_SCHEMA_VERSION: i64 = 1;

pub async fn run_migrations(pool: &SqlitePool) -> MemoryResult<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_schema_meta (\
         singleton INTEGER PRIMARY KEY CHECK (singleton = 1), \
         version INTEGER NOT NULL)",
    )
    .execute(pool)
    .await
    .map_err(|_| unavailable())?;

    let stored_version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM memory_schema_meta WHERE singleton = 1")
            .fetch_optional(pool)
            .await
            .map_err(|_| unavailable())?;
    if stored_version.is_some_and(|version| version != MEMORY_SQLITE_SCHEMA_VERSION) {
        return Err(MemoryError::ProviderUnavailable("memory schema version"));
    }

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_records (\
         app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, id TEXT NOT NULL, \
         schema_version INTEGER NOT NULL, kind TEXT NOT NULL, value_json TEXT NOT NULL, \
         evidence_json TEXT NOT NULL, confidence_bp INTEGER NOT NULL, sensitivity_json TEXT NOT NULL, \
         retention_json TEXT NOT NULL, state TEXT NOT NULL, version INTEGER NOT NULL, \
         conflict_key TEXT, supersedes_id TEXT, superseded_by_id TEXT, tombstone_json TEXT, \
         created_at TEXT NOT NULL, updated_at TEXT NOT NULL, expires_at TEXT, retention_session_id TEXT, \
         PRIMARY KEY (app_id, tenant_id, user_id, id))",
    )
    .execute(pool)
    .await
    .map_err(|_| unavailable())?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS memory_search (\
         app_id TEXT NOT NULL, tenant_id TEXT NOT NULL, user_id TEXT NOT NULL, \
         memory_id TEXT NOT NULL, search_text TEXT NOT NULL, \
         PRIMARY KEY (app_id, tenant_id, user_id, memory_id), \
         FOREIGN KEY (app_id, tenant_id, user_id, memory_id) \
         REFERENCES memory_records(app_id, tenant_id, user_id, id) ON DELETE CASCADE)",
    )
    .execute(pool)
    .await
    .map_err(|_| unavailable())?;

    for statement in [
        "CREATE INDEX IF NOT EXISTS memory_records_scope_state ON memory_records(app_id, tenant_id, user_id, state, updated_at)",
        "CREATE INDEX IF NOT EXISTS memory_records_expiry ON memory_records(app_id, tenant_id, user_id, expires_at)",
        "CREATE INDEX IF NOT EXISTS memory_records_session ON memory_records(app_id, tenant_id, user_id, retention_session_id)",
        "CREATE INDEX IF NOT EXISTS memory_records_conflict ON memory_records(app_id, tenant_id, user_id, kind, conflict_key, state)",
        "CREATE INDEX IF NOT EXISTS memory_search_scope_text ON memory_search(app_id, tenant_id, user_id, search_text)",
    ] {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(|_| unavailable())?;
    }

    sqlx::query(
        "INSERT INTO memory_schema_meta(singleton, version) VALUES(1, ?) \
         ON CONFLICT(singleton) DO NOTHING",
    )
    .bind(MEMORY_SQLITE_SCHEMA_VERSION)
    .execute(pool)
    .await
    .map_err(|_| unavailable())?;
    Ok(())
}

fn unavailable() -> MemoryError {
    MemoryError::ProviderUnavailable("memory migration")
}
