use crate::memory::{
    MEMORY_CONTRACT_SCHEMA_VERSION, MemoryConfidence, MemoryDraft, MemoryError, MemoryId,
    MemoryKind, MemoryRecord, MemoryResult, MemoryScope, MemoryState, MemoryTombstone,
    MemoryTombstoneReason, MemoryUpdate, MemoryValue,
};
use chrono::{DateTime, Utc};
use icu_casemap::CaseMapper;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::sqlite::SqliteRow;
use sqlx::{Executor, QueryBuilder, Row, Sqlite, SqlitePool};
use unicode_normalization::UnicodeNormalization;

pub(super) fn draft_to_record(
    scope: MemoryScope,
    draft: MemoryDraft,
    now: DateTime<Utc>,
) -> MemoryRecord {
    MemoryRecord {
        schema_version: MEMORY_CONTRACT_SCHEMA_VERSION,
        id: MemoryId::new(),
        scope,
        kind: draft.kind,
        value: draft.value,
        evidence: draft.evidence,
        confidence: draft.confidence,
        sensitivity: draft.sensitivity,
        retention: draft.retention,
        state: MemoryState::Proposed,
        version: 1,
        conflict_key: draft.conflict_key,
        supersedes: draft.supersedes,
        superseded_by: None,
        tombstone: None,
        created_at: now,
        updated_at: now,
    }
}

pub(super) async fn insert_record(
    connection: &mut sqlx::SqliteConnection,
    record: &MemoryRecord,
) -> MemoryResult<()> {
    sqlx::query(
        "INSERT INTO memory_records (app_id, tenant_id, user_id, id, schema_version, kind, \
         value_json, evidence_json, confidence_bp, sensitivity_json, retention_json, state, \
         version, conflict_key, supersedes_id, superseded_by_id, tombstone_json, created_at, \
         updated_at, expires_at, retention_session_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&record.scope.app_id)
    .bind(&record.scope.tenant_id)
    .bind(&record.scope.user_id)
    .bind(record.id.as_str())
    .bind(i64::from(record.schema_version))
    .bind(record.kind.as_str())
    .bind(to_json(&record.value, "insert")?)
    .bind(to_json(&record.evidence, "insert")?)
    .bind(i64::from(record.confidence.basis_points()))
    .bind(to_json(&record.sensitivity, "insert")?)
    .bind(to_json(&record.retention, "insert")?)
    .bind(record.state.as_str())
    .bind(version_to_i64(record.version)?)
    .bind(&record.conflict_key)
    .bind(record.supersedes.as_ref().map(MemoryId::as_str))
    .bind(record.superseded_by.as_ref().map(MemoryId::as_str))
    .bind(to_optional_json(record.tombstone.as_ref(), "insert")?)
    .bind(record.created_at.to_rfc3339())
    .bind(record.updated_at.to_rfc3339())
    .bind(
        record
            .retention
            .expires_at()
            .map(|value| value.to_rfc3339()),
    )
    .bind(record.retention.session_id())
    .execute(connection)
    .await
    .map_err(|_| unavailable("insert"))?;
    Ok(())
}

pub(super) async fn fetch_record<'e, E>(
    executor: E,
    scope: &MemoryScope,
    id: &MemoryId,
) -> MemoryResult<Option<MemoryRecord>>
where
    E: Executor<'e, Database = Sqlite>,
{
    let row = sqlx::query(
        "SELECT * FROM memory_records WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ?",
    )
    .bind(&scope.app_id)
    .bind(&scope.tenant_id)
    .bind(&scope.user_id)
    .bind(id.as_str())
    .fetch_optional(executor)
    .await
    .map_err(|_| unavailable("fetch"))?;
    row.as_ref().map(row_to_record).transpose()
}

pub(super) fn row_to_record(row: &SqliteRow) -> MemoryResult<MemoryRecord> {
    let schema_version =
        u32::try_from(read_i64(row, "schema_version")?).map_err(|_| unavailable("decode"))?;
    let confidence =
        u16::try_from(read_i64(row, "confidence_bp")?).map_err(|_| unavailable("decode"))?;
    let version = u64::try_from(read_i64(row, "version")?).map_err(|_| unavailable("decode"))?;
    let state = match read_string(row, "state")?.as_str() {
        "proposed" => MemoryState::Proposed,
        "committed" => MemoryState::Committed,
        "tombstoned" => MemoryState::Tombstoned,
        _ => return Err(unavailable("decode")),
    };
    let record = MemoryRecord {
        schema_version,
        id: MemoryId::parse(&read_string(row, "id")?)?,
        scope: MemoryScope::new(
            read_string(row, "app_id")?,
            read_string(row, "tenant_id")?,
            read_string(row, "user_id")?,
        )?,
        kind: MemoryKind::parse(&read_string(row, "kind")?)?,
        value: from_json(&read_string(row, "value_json")?, "decode")?,
        evidence: from_json(&read_string(row, "evidence_json")?, "decode")?,
        confidence: MemoryConfidence::from_basis_points(confidence)?,
        sensitivity: from_json(&read_string(row, "sensitivity_json")?, "decode")?,
        retention: from_json(&read_string(row, "retention_json")?, "decode")?,
        state,
        version,
        conflict_key: read_optional_string(row, "conflict_key")?,
        supersedes: parse_optional_id(read_optional_string(row, "supersedes_id")?)?,
        superseded_by: parse_optional_id(read_optional_string(row, "superseded_by_id")?)?,
        tombstone: from_optional_json(read_optional_string(row, "tombstone_json")?, "decode")?,
        created_at: parse_timestamp(&read_string(row, "created_at")?)?,
        updated_at: parse_timestamp(&read_string(row, "updated_at")?)?,
    };
    record.validate()?;
    Ok(record)
}

pub(super) async fn upsert_search_entry(
    connection: &mut sqlx::SqliteConnection,
    record: &MemoryRecord,
) -> MemoryResult<()> {
    let mut source = record.value.text.clone();
    for value in record.value.attributes.values() {
        source.push(' ');
        source.push_str(value);
    }
    sqlx::query(
        "INSERT INTO memory_search(app_id, tenant_id, user_id, memory_id, search_text) \
         VALUES(?, ?, ?, ?, ?) ON CONFLICT(app_id, tenant_id, user_id, memory_id) \
         DO UPDATE SET search_text = excluded.search_text",
    )
    .bind(&record.scope.app_id)
    .bind(&record.scope.tenant_id)
    .bind(&record.scope.user_id)
    .bind(record.id.as_str())
    .bind(normalize_search_text(&source))
    .execute(connection)
    .await
    .map_err(|_| unavailable("index"))?;
    Ok(())
}

pub(super) async fn delete_search_entry(
    connection: &mut sqlx::SqliteConnection,
    scope: &MemoryScope,
    id: &MemoryId,
) -> MemoryResult<()> {
    sqlx::query(
        "DELETE FROM memory_search WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND memory_id = ?",
    )
    .bind(&scope.app_id)
    .bind(&scope.tenant_id)
    .bind(&scope.user_id)
    .bind(id.as_str())
    .execute(connection)
    .await
    .map_err(|_| unavailable("delete index"))?;
    Ok(())
}

pub(super) async fn conflict_ids(
    connection: &mut sqlx::SqliteConnection,
    record: &MemoryRecord,
    now: DateTime<Utc>,
) -> MemoryResult<Vec<MemoryId>> {
    let Some(conflict_key) = &record.conflict_key else {
        return Ok(Vec::new());
    };
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM memory_records WHERE app_id = ? AND tenant_id = ? AND user_id = ? \
         AND kind = ? AND conflict_key = ? AND id != ? AND state = 'committed' AND superseded_by_id IS NULL \
         AND (expires_at IS NULL OR expires_at > ?) ORDER BY id ASC",
    )
    .bind(&record.scope.app_id)
    .bind(&record.scope.tenant_id)
    .bind(&record.scope.user_id)
    .bind(record.kind.as_str())
    .bind(conflict_key)
    .bind(record.id.as_str())
    .bind(now.to_rfc3339())
    .fetch_all(connection)
    .await
    .map_err(|_| unavailable("conflict lookup"))?;
    ids.iter().map(|id| MemoryId::parse(id)).collect()
}

pub(super) async fn tombstone_matching(
    pool: &SqlitePool,
    scope: &MemoryScope,
    predicate: &str,
    value: Option<String>,
    reason: MemoryTombstoneReason,
    now: DateTime<Utc>,
) -> MemoryResult<u64> {
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM memory_records WHERE app_id = ");
    builder
        .push_bind(&scope.app_id)
        .push(" AND tenant_id = ")
        .push_bind(&scope.tenant_id)
        .push(" AND user_id = ")
        .push_bind(&scope.user_id)
        .push(" AND state != 'tombstoned' AND ");
    match predicate {
        "expires_at IS NOT NULL AND expires_at <= ?" => {
            builder.push("expires_at IS NOT NULL AND expires_at <= ");
        }
        "retention_session_id = ?" => {
            builder.push("retention_session_id = ");
        }
        _ => {
            return Err(MemoryError::InvalidInput(
                "invalid retention predicate".into(),
            ));
        }
    }
    builder.push_bind(value.ok_or_else(|| unavailable("retention"))?);
    let rows = builder
        .build()
        .fetch_all(pool)
        .await
        .map_err(|_| unavailable("retention"))?;
    if rows.is_empty() {
        return Ok(0);
    }

    let records = rows
        .iter()
        .map(row_to_record)
        .collect::<MemoryResult<Vec<_>>>()?;
    let tombstone = MemoryTombstone { reason, at: now };
    let redacted = MemoryValue::redacted();
    let mut transaction = pool.begin().await.map_err(|_| unavailable("retention"))?;
    let mut affected = 0_u64;
    for record in records {
        let rows = sqlx::query(
            "UPDATE memory_records SET value_json = ?, evidence_json = '[]', state = 'tombstoned', \
             version = version + 1, tombstone_json = ?, updated_at = ? \
             WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ? AND version = ? AND state != 'tombstoned'",
        )
        .bind(to_json(&redacted, "retention")?)
        .bind(to_json(&tombstone, "retention")?)
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(record.id.as_str())
        .bind(version_to_i64(record.version)?)
        .execute(&mut *transaction)
        .await
        .map_err(|_| unavailable("retention"))?
        .rows_affected();
        if rows == 1 {
            delete_search_entry(&mut transaction, scope, &record.id).await?;
            affected += 1;
        }
    }
    transaction
        .commit()
        .await
        .map_err(|_| unavailable("retention"))?;
    Ok(affected)
}

pub(super) fn ensure_live_committed(
    record: &MemoryRecord,
    _label: &'static str,
) -> MemoryResult<()> {
    ensure_state(record, MemoryState::Committed)?;
    if record.superseded_by.is_some() || record.is_expired_at(Utc::now()) {
        return Err(MemoryError::InvalidState {
            id: record.id.as_str().into(),
            expected: "live committed",
            actual: "inactive",
        });
    }
    Ok(())
}

pub(super) fn ensure_state(record: &MemoryRecord, expected: MemoryState) -> MemoryResult<()> {
    if record.state != expected {
        return Err(MemoryError::InvalidState {
            id: record.id.as_str().into(),
            expected: expected.as_str(),
            actual: record.state.as_str(),
        });
    }
    Ok(())
}

pub(super) fn ensure_version(record: &MemoryRecord, expected: u64) -> MemoryResult<()> {
    if record.version != expected {
        return Err(MemoryError::VersionConflict {
            id: record.id.as_str().into(),
            expected,
            actual: record.version,
        });
    }
    Ok(())
}

pub(super) fn ensure_cas_affected(
    affected: u64,
    record: &MemoryRecord,
    expected: u64,
) -> MemoryResult<()> {
    if affected != 1 {
        return Err(MemoryError::VersionConflict {
            id: record.id.as_str().into(),
            expected,
            actual: record.version.max(expected.saturating_add(1)),
        });
    }
    Ok(())
}

pub(super) fn apply_update(record: &mut MemoryRecord, update: MemoryUpdate) {
    if let Some(value) = update.value {
        record.value = value;
    }
    if let Some(evidence) = update.evidence {
        record.evidence = evidence;
    }
    if let Some(confidence) = update.confidence {
        record.confidence = confidence;
    }
    if let Some(sensitivity) = update.sensitivity {
        record.sensitivity = sensitivity;
    }
    if let Some(retention) = update.retention {
        record.retention = retention;
    }
    if let Some(conflict_key) = update.conflict_key {
        record.conflict_key = conflict_key;
    }
}

pub(super) fn normalize_search_text(value: &str) -> String {
    let normalized = value.nfkc().collect::<String>();
    CaseMapper::new()
        .fold_string(&normalized)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn version_to_i64(value: u64) -> MemoryResult<i64> {
    i64::try_from(value)
        .map_err(|_| MemoryError::InvalidInput("memory version is too large".into()))
}

pub(super) fn to_json<T: Serialize>(value: &T, operation: &'static str) -> MemoryResult<String> {
    serde_json::to_string(value).map_err(|_| unavailable(operation))
}

fn to_optional_json<T: Serialize>(
    value: Option<&T>,
    operation: &'static str,
) -> MemoryResult<Option<String>> {
    value.map(|value| to_json(value, operation)).transpose()
}

fn from_json<T: DeserializeOwned>(value: &str, operation: &'static str) -> MemoryResult<T> {
    serde_json::from_str(value).map_err(|_| unavailable(operation))
}

fn from_optional_json<T: DeserializeOwned>(
    value: Option<String>,
    operation: &'static str,
) -> MemoryResult<Option<T>> {
    value.map(|value| from_json(&value, operation)).transpose()
}

fn parse_optional_id(value: Option<String>) -> MemoryResult<Option<MemoryId>> {
    value.map(|value| MemoryId::parse(&value)).transpose()
}

fn parse_timestamp(value: &str) -> MemoryResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| unavailable("decode"))
}

fn read_string(row: &SqliteRow, name: &'static str) -> MemoryResult<String> {
    row.try_get(name).map_err(|_| unavailable("decode"))
}

fn read_optional_string(row: &SqliteRow, name: &'static str) -> MemoryResult<Option<String>> {
    row.try_get(name).map_err(|_| unavailable("decode"))
}

fn read_i64(row: &SqliteRow, name: &'static str) -> MemoryResult<i64> {
    row.try_get(name).map_err(|_| unavailable("decode"))
}

fn unavailable(operation: &'static str) -> MemoryError {
    MemoryError::ProviderUnavailable(operation)
}
