use crate::memory::{
    ExplicitMemoryMutation, MEMORY_CONTRACT_SCHEMA_VERSION, MemoryCandidateBatch,
    MemoryDeleteResult, MemoryDraft, MemoryError, MemoryExport, MemoryExportRequest, MemoryId,
    MemoryMutationAction, MemoryMutationRequest, MemoryMutationResult, MemoryProvider,
    MemoryRecallRequest, MemoryRecord, MemoryResult, MemoryScope, MemoryState, MemoryTombstone,
    MemoryTombstoneReason, MemoryUpdate, MemoryValue,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use std::str::FromStr;
use std::time::Duration;

#[path = "memory_sqlite_schema.rs"]
mod schema;
#[path = "memory_sqlite_support.rs"]
mod support;

pub use schema::MEMORY_SQLITE_SCHEMA_VERSION;
use support::*;

#[derive(Clone)]
pub struct SqliteMemoryProvider {
    pool: SqlitePool,
}

impl SqliteMemoryProvider {
    pub async fn connect(url: &str) -> MemoryResult<Self> {
        let options = SqliteConnectOptions::from_str(url)
            .map_err(|_| unavailable("connect"))?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|_| unavailable("connect"))?;
        let provider = Self { pool };
        if let Err(error) = provider.initialize().await {
            provider.pool.close().await;
            return Err(error);
        }
        Ok(provider)
    }

    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn get(
        &self,
        scope: &MemoryScope,
        id: &MemoryId,
        include_tombstone: bool,
    ) -> MemoryResult<Option<MemoryRecord>> {
        scope.validate()?;
        self.expire_due(scope, Utc::now()).await?;
        let record = fetch_record(&self.pool, scope, id).await?;
        Ok(record.filter(|record| include_tombstone || record.state != MemoryState::Tombstoned))
    }

    pub async fn propose(
        &self,
        scope: MemoryScope,
        draft: MemoryDraft,
    ) -> MemoryResult<MemoryMutationResult> {
        self.mutate(MemoryMutationRequest {
            scope,
            mutation: ExplicitMemoryMutation::Propose { draft },
        })
        .await
    }

    pub async fn confirm(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
    ) -> MemoryResult<MemoryMutationResult> {
        self.mutate(MemoryMutationRequest {
            scope,
            mutation: ExplicitMemoryMutation::Confirm {
                id,
                expected_version,
            },
        })
        .await
    }

    pub async fn update(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
        changes: MemoryUpdate,
    ) -> MemoryResult<MemoryMutationResult> {
        self.mutate(MemoryMutationRequest {
            scope,
            mutation: ExplicitMemoryMutation::Update {
                id,
                expected_version,
                changes,
            },
        })
        .await
    }

    pub async fn forget(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
        reason: MemoryTombstoneReason,
    ) -> MemoryResult<MemoryMutationResult> {
        self.mutate(MemoryMutationRequest {
            scope,
            mutation: ExplicitMemoryMutation::Forget {
                id,
                expected_version,
                reason,
            },
        })
        .await
    }

    async fn run_migrations(&self) -> MemoryResult<()> {
        schema::run_migrations(&self.pool).await
    }

    async fn propose_inner(
        &self,
        scope: MemoryScope,
        draft: MemoryDraft,
    ) -> MemoryResult<MemoryMutationResult> {
        scope.validate()?;
        draft.validate()?;
        let now = Utc::now();
        if draft
            .retention
            .expires_at()
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(MemoryError::InvalidInput(
                "cannot propose already-expired memory".into(),
            ));
        }
        if let Some(supersedes) = &draft.supersedes {
            let target = fetch_record(&self.pool, &scope, supersedes)
                .await?
                .ok_or_else(|| MemoryError::NotFound(supersedes.as_str().into()))?;
            ensure_live_committed(&target, "supersession target")?;
        }
        let record = draft_to_record(scope, draft, now);
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("propose"))?;
        insert_record(&mut transaction, &record).await?;
        transaction
            .commit()
            .await
            .map_err(|_| unavailable("propose"))?;
        Ok(MemoryMutationResult {
            action: MemoryMutationAction::Proposed,
            record,
            conflicts: Vec::new(),
        })
    }

    async fn confirm_inner(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
    ) -> MemoryResult<MemoryMutationResult> {
        scope.validate()?;
        let expected = version_to_i64(expected_version)?;
        let now = Utc::now();
        let mut record = fetch_record(&self.pool, &scope, &id)
            .await?
            .ok_or_else(|| MemoryError::NotFound(id.as_str().into()))?;
        ensure_version(&record, expected_version)?;
        ensure_state(&record, MemoryState::Proposed)?;
        if record.is_expired_at(now) {
            return Err(MemoryError::InvalidInput(
                "cannot confirm expired memory".into(),
            ));
        }
        let supersedes = record.supersedes.clone();
        let supersession_target = if let Some(supersedes) = &supersedes {
            let target = fetch_record(&self.pool, &scope, supersedes)
                .await?
                .ok_or_else(|| MemoryError::NotFound(supersedes.as_str().into()))?;
            ensure_live_committed(&target, "supersession target")?;
            Some(target)
        } else {
            None
        };
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("confirm"))?;

        if let (Some(supersedes), Some(target)) = (&supersedes, &supersession_target) {
            let affected = sqlx::query(
                "UPDATE memory_records SET superseded_by_id = ?, version = version + 1, updated_at = ? WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ? AND version = ? AND state = 'committed'",
            )
            .bind(id.as_str())
            .bind(now.to_rfc3339())
            .bind(&scope.app_id)
            .bind(&scope.tenant_id)
            .bind(&scope.user_id)
            .bind(supersedes.as_str())
            .bind(version_to_i64(target.version)?)
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable("confirm"))?
            .rows_affected();
            if affected != 1 {
                return Err(MemoryError::VersionConflict {
                    id: supersedes.as_str().into(),
                    expected: target.version,
                    actual: target.version.saturating_add(1),
                });
            }
            delete_search_entry(&mut transaction, &scope, supersedes).await?;
        }

        let affected = sqlx::query(
            "UPDATE memory_records SET state = 'committed', version = version + 1, updated_at = ? WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ? AND version = ? AND state = 'proposed'",
        )
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(id.as_str())
        .bind(expected)
        .execute(&mut *transaction)
        .await
        .map_err(|_| unavailable("confirm"))?
        .rows_affected();
        ensure_cas_affected(affected, &record, expected_version)?;
        record.state = MemoryState::Committed;
        record.version += 1;
        record.updated_at = now;
        upsert_search_entry(&mut transaction, &record).await?;
        let conflicts = conflict_ids(&mut transaction, &record, now).await?;
        transaction
            .commit()
            .await
            .map_err(|_| unavailable("confirm"))?;
        Ok(MemoryMutationResult {
            action: MemoryMutationAction::Confirmed,
            record,
            conflicts: conflicts
                .into_iter()
                .filter(|conflict| Some(conflict) != supersedes.as_ref())
                .collect(),
        })
    }

    async fn update_inner(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
        changes: MemoryUpdate,
    ) -> MemoryResult<MemoryMutationResult> {
        scope.validate()?;
        changes.validate()?;
        let expected = version_to_i64(expected_version)?;
        let now = Utc::now();
        let mut record = fetch_record(&self.pool, &scope, &id)
            .await?
            .ok_or_else(|| MemoryError::NotFound(id.as_str().into()))?;
        ensure_version(&record, expected_version)?;
        if record.state == MemoryState::Tombstoned {
            return Err(MemoryError::InvalidState {
                id: id.as_str().into(),
                expected: "proposed or committed",
                actual: record.state.as_str(),
            });
        }
        apply_update(&mut record, changes);
        record.version += 1;
        record.updated_at = now;
        record.validate()?;

        let mut transaction = self.pool.begin().await.map_err(|_| unavailable("update"))?;
        let affected = sqlx::query(
            "UPDATE memory_records SET value_json = ?, evidence_json = ?, confidence_bp = ?, sensitivity_json = ?, retention_json = ?, expires_at = ?, retention_session_id = ?, conflict_key = ?, version = ?, updated_at = ? WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ? AND version = ? AND state != 'tombstoned'",
        )
        .bind(to_json(&record.value, "update")?)
        .bind(to_json(&record.evidence, "update")?)
        .bind(i64::from(record.confidence.basis_points()))
        .bind(to_json(&record.sensitivity, "update")?)
        .bind(to_json(&record.retention, "update")?)
        .bind(record.retention.expires_at().map(|value| value.to_rfc3339()))
        .bind(record.retention.session_id())
        .bind(&record.conflict_key)
        .bind(version_to_i64(record.version)?)
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(id.as_str())
        .bind(expected)
        .execute(&mut *transaction)
        .await
        .map_err(|_| unavailable("update"))?
        .rows_affected();
        ensure_cas_affected(affected, &record, expected_version)?;

        if record.state == MemoryState::Committed && !record.is_expired_at(now) {
            upsert_search_entry(&mut transaction, &record).await?;
        } else {
            delete_search_entry(&mut transaction, &scope, &id).await?;
        }
        let conflicts = if record.state == MemoryState::Committed {
            conflict_ids(&mut transaction, &record, now).await?
        } else {
            Vec::new()
        };
        transaction
            .commit()
            .await
            .map_err(|_| unavailable("update"))?;
        Ok(MemoryMutationResult {
            action: MemoryMutationAction::Updated,
            record,
            conflicts,
        })
    }

    async fn forget_inner(
        &self,
        scope: MemoryScope,
        id: MemoryId,
        expected_version: u64,
        reason: MemoryTombstoneReason,
    ) -> MemoryResult<MemoryMutationResult> {
        scope.validate()?;
        let expected = version_to_i64(expected_version)?;
        let now = Utc::now();
        let tombstone = MemoryTombstone { reason, at: now };
        let redacted = MemoryValue::redacted();
        let mut record = fetch_record(&self.pool, &scope, &id)
            .await?
            .ok_or_else(|| MemoryError::NotFound(id.as_str().into()))?;
        ensure_version(&record, expected_version)?;
        if record.state == MemoryState::Tombstoned {
            return Err(MemoryError::InvalidState {
                id: id.as_str().into(),
                expected: "live",
                actual: record.state.as_str(),
            });
        }
        let mut transaction = self.pool.begin().await.map_err(|_| unavailable("forget"))?;
        let affected = sqlx::query(
            "UPDATE memory_records SET value_json = ?, evidence_json = '[]', state = 'tombstoned', version = version + 1, tombstone_json = ?, updated_at = ? WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ? AND version = ? AND state != 'tombstoned'",
        )
        .bind(to_json(&redacted, "forget")?)
        .bind(to_json(&tombstone, "forget")?)
        .bind(now.to_rfc3339())
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .bind(id.as_str())
        .bind(expected)
        .execute(&mut *transaction)
        .await
        .map_err(|_| unavailable("forget"))?
        .rows_affected();
        ensure_cas_affected(affected, &record, expected_version)?;
        delete_search_entry(&mut transaction, &scope, &id).await?;
        record.value = redacted;
        record.evidence.clear();
        record.state = MemoryState::Tombstoned;
        record.version += 1;
        record.tombstone = Some(tombstone);
        record.updated_at = now;
        record.validate()?;
        transaction
            .commit()
            .await
            .map_err(|_| unavailable("forget"))?;
        Ok(MemoryMutationResult {
            action: MemoryMutationAction::Forgotten,
            record,
            conflicts: Vec::new(),
        })
    }

    async fn recall_inner(&self, request: MemoryRecallRequest) -> MemoryResult<Vec<MemoryRecord>> {
        request.validate()?;
        let now = Utc::now();
        self.expire_due(&request.scope, now).await?;
        let normalized_query = normalize_search_text(&request.query);
        let mut builder = QueryBuilder::<Sqlite>::new("SELECT r.* FROM memory_records r ");
        if !normalized_query.is_empty() {
            builder.push("JOIN memory_search s ON s.app_id = r.app_id AND s.tenant_id = r.tenant_id AND s.user_id = r.user_id AND s.memory_id = r.id ");
        }
        builder
            .push("WHERE r.app_id = ")
            .push_bind(&request.scope.app_id)
            .push(" AND r.tenant_id = ")
            .push_bind(&request.scope.tenant_id)
            .push(" AND r.user_id = ")
            .push_bind(&request.scope.user_id)
            .push(" AND r.state = 'committed' AND r.superseded_by_id IS NULL AND (r.expires_at IS NULL OR r.expires_at > ")
            .push_bind(now.to_rfc3339())
            .push(") ");
        if !request.kinds.is_empty() {
            builder.push("AND r.kind IN (");
            let mut separated = builder.separated(", ");
            for kind in &request.kinds {
                separated.push_bind(kind.as_str());
            }
            separated.push_unseparated(") ");
        }
        if !normalized_query.is_empty() {
            builder
                .push("AND instr(s.search_text, ")
                .push_bind(&normalized_query)
                .push(") > 0 ORDER BY CASE WHEN s.search_text = ")
                .push_bind(&normalized_query)
                .push(" THEN 0 ELSE 1 END, r.updated_at DESC, r.id ASC ");
        } else {
            builder.push("ORDER BY r.updated_at DESC, r.id ASC ");
        }
        builder.push("LIMIT ").push_bind(request.limit as i64);
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| unavailable("recall"))?;
        rows.iter().map(row_to_record).collect()
    }

    async fn persist_candidates(
        &self,
        batch: MemoryCandidateBatch,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        batch.validate()?;
        let mut records = Vec::with_capacity(batch.candidates.len());
        for candidate in batch.candidates {
            records.push(
                self.propose_inner(batch.scope.clone(), candidate)
                    .await?
                    .record,
            );
        }
        Ok(records)
    }

    async fn expire_due(&self, scope: &MemoryScope, now: DateTime<Utc>) -> MemoryResult<u64> {
        tombstone_matching(
            &self.pool,
            scope,
            "expires_at IS NOT NULL AND expires_at <= ?",
            Some(now.to_rfc3339()),
            MemoryTombstoneReason::Expired,
            now,
        )
        .await
    }

    async fn expire_session(
        &self,
        scope: &MemoryScope,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> MemoryResult<u64> {
        tombstone_matching(
            &self.pool,
            scope,
            "retention_session_id = ?",
            Some(session_id.to_string()),
            MemoryTombstoneReason::SessionEnded,
            now,
        )
        .await
    }
}

#[async_trait]
impl MemoryProvider for SqliteMemoryProvider {
    async fn initialize(&self) -> MemoryResult<()> {
        self.run_migrations().await
    }

    async fn get(
        &self,
        request: crate::memory::MemoryGetRequest,
    ) -> MemoryResult<Option<MemoryRecord>> {
        SqliteMemoryProvider::get(self, &request.scope, &request.id, request.include_tombstone)
            .await
    }

    async fn pre_turn_recall(
        &self,
        request: MemoryRecallRequest,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        self.recall_inner(request).await
    }

    async fn post_turn_candidates(
        &self,
        batch: MemoryCandidateBatch,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        self.persist_candidates(batch).await
    }

    async fn on_session_end(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>> {
        let scope = batch.scope.clone();
        let session_id = batch.session_id.clone();
        let records = self.persist_candidates(batch).await?;
        self.expire_session(&scope, &session_id, Utc::now()).await?;
        Ok(records)
    }

    async fn on_compaction(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>> {
        self.persist_candidates(batch).await
    }

    async fn mutate(&self, request: MemoryMutationRequest) -> MemoryResult<MemoryMutationResult> {
        match request.mutation {
            ExplicitMemoryMutation::Propose { draft } => {
                self.propose_inner(request.scope, draft).await
            }
            ExplicitMemoryMutation::Confirm {
                id,
                expected_version,
            } => {
                self.confirm_inner(request.scope, id, expected_version)
                    .await
            }
            ExplicitMemoryMutation::Update {
                id,
                expected_version,
                changes,
            } => {
                self.update_inner(request.scope, id, expected_version, changes)
                    .await
            }
            ExplicitMemoryMutation::Forget {
                id,
                expected_version,
                reason,
            } => {
                self.forget_inner(request.scope, id, expected_version, reason)
                    .await
            }
        }
    }

    async fn export(&self, request: MemoryExportRequest) -> MemoryResult<MemoryExport> {
        request.scope.validate()?;
        self.expire_due(&request.scope, Utc::now()).await?;
        let mut builder =
            QueryBuilder::<Sqlite>::new("SELECT * FROM memory_records WHERE app_id = ");
        builder
            .push_bind(&request.scope.app_id)
            .push(" AND tenant_id = ")
            .push_bind(&request.scope.tenant_id)
            .push(" AND user_id = ")
            .push_bind(&request.scope.user_id);
        if !request.include_proposals {
            builder.push(" AND state != 'proposed'");
        }
        if !request.include_tombstones {
            builder.push(" AND state != 'tombstoned'");
        }
        builder.push(" ORDER BY id ASC");
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|_| unavailable("export"))?;
        let records = rows
            .iter()
            .map(row_to_record)
            .collect::<MemoryResult<Vec<_>>>()?;
        Ok(MemoryExport {
            schema_version: MEMORY_CONTRACT_SCHEMA_VERSION,
            scope: request.scope,
            exported_at: Utc::now(),
            records,
        })
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> MemoryResult<MemoryDeleteResult> {
        scope.validate()?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| unavailable("delete scope"))?;
        let indexed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memory_search WHERE app_id = ? AND tenant_id = ? AND user_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| unavailable("delete scope"))?;
        sqlx::query("DELETE FROM memory_search WHERE app_id = ? AND tenant_id = ? AND user_id = ?")
            .bind(&scope.app_id)
            .bind(&scope.tenant_id)
            .bind(&scope.user_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| unavailable("delete scope"))?;
        let deleted = sqlx::query(
            "DELETE FROM memory_records WHERE app_id = ? AND tenant_id = ? AND user_id = ?",
        )
        .bind(&scope.app_id)
        .bind(&scope.tenant_id)
        .bind(&scope.user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| unavailable("delete scope"))?
        .rows_affected();
        transaction
            .commit()
            .await
            .map_err(|_| unavailable("delete scope"))?;
        Ok(MemoryDeleteResult {
            deleted_records: deleted,
            deleted_index_entries: u64::try_from(indexed).unwrap_or(0),
        })
    }

    async fn shutdown(&self) -> MemoryResult<()> {
        self.pool.close().await;
        Ok(())
    }
}

fn unavailable(operation: &'static str) -> MemoryError {
    MemoryError::ProviderUnavailable(operation)
}

#[cfg(test)]
#[path = "memory_sqlite_tests.rs"]
mod tests;
