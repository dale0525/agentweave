use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

fn scope() -> MemoryScope {
    MemoryScope::new("com.example.secretary", "tenant-a", "user-a").unwrap()
}

fn evidence(source: MemoryEvidenceSource, excerpt: &str) -> MemoryEvidence {
    MemoryEvidence {
        source,
        source_id: Some("session-1".into()),
        excerpt: Some(excerpt.into()),
        observed_at: Utc::now(),
    }
}

fn draft(text: &str) -> MemoryDraft {
    MemoryDraft {
        kind: MemoryKind::parse(MemoryKind::PREFERENCE).unwrap(),
        value: MemoryValue::new(text).unwrap(),
        evidence: vec![evidence(MemoryEvidenceSource::UserStatement, text)],
        confidence: MemoryConfidence::from_basis_points(9500).unwrap(),
        sensitivity: MemorySensitivity::Personal,
        retention: MemoryRetention::Persistent,
        conflict_key: Some("preferred-language".into()),
        supersedes: None,
    }
}

#[test]
fn stable_identity_scope_kind_and_confidence_validation() {
    let id = MemoryId::new();
    assert_eq!(MemoryId::parse(id.as_str()).unwrap(), id);
    assert!(MemoryId::parse("not-a-uuid").is_err());

    assert!(MemoryScope::new("app", "tenant", "user").is_ok());
    assert!(MemoryScope::new("", "tenant", "user").is_err());
    assert!(MemoryKind::parse("user.preference").is_ok());
    assert!(MemoryKind::parse("Preference").is_err());
    assert!(MemoryConfidence::from_basis_points(10_000).is_ok());
    assert!(MemoryConfidence::from_basis_points(10_001).is_err());
}

#[test]
fn records_enforce_proposal_committed_and_scrubbed_tombstone_invariants() {
    let now = Utc::now();
    let mut record = MemoryRecord {
        schema_version: MEMORY_CONTRACT_SCHEMA_VERSION,
        id: MemoryId::new(),
        scope: scope(),
        kind: MemoryKind::parse(MemoryKind::USER_FACT).unwrap(),
        value: MemoryValue::new("Likes jasmine tea").unwrap(),
        evidence: vec![evidence(
            MemoryEvidenceSource::ExplicitUserAction,
            "Remember that I like jasmine tea",
        )],
        confidence: MemoryConfidence::from_basis_points(10_000).unwrap(),
        sensitivity: MemorySensitivity::Personal,
        retention: MemoryRetention::Persistent,
        state: MemoryState::Proposed,
        version: 1,
        conflict_key: None,
        supersedes: None,
        superseded_by: None,
        tombstone: None,
        created_at: now,
        updated_at: now,
    };
    record.validate().unwrap();

    record.state = MemoryState::Tombstoned;
    assert!(record.validate().is_err());
    record.value = MemoryValue::redacted();
    record.evidence.clear();
    record.tombstone = Some(MemoryTombstone {
        reason: MemoryTombstoneReason::UserRequest,
        at: now,
    });
    record.validate().unwrap();
}

#[test]
fn debug_output_redacts_memory_values_evidence_and_scope_principals() {
    let now = Utc::now();
    let secret = "private-memory-value-981723";
    let record = MemoryRecord {
        schema_version: MEMORY_CONTRACT_SCHEMA_VERSION,
        id: MemoryId::new(),
        scope: scope(),
        kind: MemoryKind::parse(MemoryKind::USER_FACT).unwrap(),
        value: MemoryValue::new(secret).unwrap(),
        evidence: vec![evidence(MemoryEvidenceSource::UserStatement, secret)],
        confidence: MemoryConfidence::from_basis_points(9000).unwrap(),
        sensitivity: MemorySensitivity::Sensitive,
        retention: MemoryRetention::Persistent,
        state: MemoryState::Committed,
        version: 1,
        conflict_key: Some("private-key".into()),
        supersedes: None,
        superseded_by: None,
        tombstone: None,
        created_at: now,
        updated_at: now,
    };

    let debug = format!("{record:?}");

    assert!(!debug.contains(secret));
    assert!(!debug.contains("tenant-a"));
    assert!(!debug.contains("user-a"));
    assert!(debug.contains("[redacted]"));
}

#[test]
fn expiry_and_session_retention_are_explicit() {
    let now = Utc::now();
    let mut record = committed_record(draft("temporary"), scope());
    record.retention = MemoryRetention::ExpiresAt { expires_at: now };
    assert!(record.is_expired_at(now));

    let retention = MemoryRetention::Session {
        session_id: "session-42".into(),
    };
    assert_eq!(retention.session_id(), Some("session-42"));
    assert_eq!(retention.expires_at(), None);
}

struct FakeMemoryProvider {
    initialized: AtomicBool,
    records: Mutex<BTreeMap<MemoryId, MemoryRecord>>,
}

impl FakeMemoryProvider {
    fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            records: Mutex::new(BTreeMap::new()),
        }
    }

    fn ensure_initialized(&self) -> MemoryResult<()> {
        if self.initialized.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(MemoryError::ProviderUnavailable("fake provider operation"))
        }
    }

    fn propose_one(&self, scope: MemoryScope, draft: MemoryDraft) -> MemoryResult<MemoryRecord> {
        draft.validate()?;
        let now = Utc::now();
        let record = MemoryRecord {
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
        };
        self.records
            .lock()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    fn candidate_batch(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>> {
        batch.validate()?;
        batch
            .candidates
            .into_iter()
            .map(|candidate| self.propose_one(batch.scope.clone(), candidate))
            .collect()
    }
}

#[async_trait]
impl MemoryProvider for FakeMemoryProvider {
    async fn initialize(&self) -> MemoryResult<()> {
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn get(&self, request: MemoryGetRequest) -> MemoryResult<Option<MemoryRecord>> {
        self.ensure_initialized()?;
        request.scope.validate()?;
        Ok(self
            .records
            .lock()
            .unwrap()
            .get(&request.id)
            .filter(|record| record.scope == request.scope)
            .filter(|record| request.include_tombstone || record.state != MemoryState::Tombstoned)
            .cloned())
    }

    async fn pre_turn_recall(
        &self,
        request: MemoryRecallRequest,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        self.ensure_initialized()?;
        request.validate()?;
        let query = request.query.to_lowercase();
        let records = self.records.lock().unwrap();
        Ok(records
            .values()
            .filter(|record| record.scope == request.scope)
            .filter(|record| record.state == MemoryState::Committed)
            .filter(|record| record.superseded_by.is_none())
            .filter(|record| request.kinds.is_empty() || request.kinds.contains(&record.kind))
            .filter(|record| query.is_empty() || record.value.text.to_lowercase().contains(&query))
            .take(request.limit)
            .cloned()
            .collect())
    }

    async fn post_turn_candidates(
        &self,
        batch: MemoryCandidateBatch,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        self.ensure_initialized()?;
        self.candidate_batch(batch)
    }

    async fn on_session_end(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>> {
        self.ensure_initialized()?;
        self.candidate_batch(batch)
    }

    async fn on_compaction(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>> {
        self.ensure_initialized()?;
        self.candidate_batch(batch)
    }

    async fn mutate(&self, request: MemoryMutationRequest) -> MemoryResult<MemoryMutationResult> {
        self.ensure_initialized()?;
        request.scope.validate()?;
        if let ExplicitMemoryMutation::Propose { draft } = request.mutation {
            let record = self.propose_one(request.scope, draft)?;
            return Ok(MemoryMutationResult {
                action: MemoryMutationAction::Proposed,
                record,
                conflicts: Vec::new(),
            });
        }

        let (id, expected_version) = match &request.mutation {
            ExplicitMemoryMutation::Confirm {
                id,
                expected_version,
            }
            | ExplicitMemoryMutation::Update {
                id,
                expected_version,
                ..
            }
            | ExplicitMemoryMutation::Forget {
                id,
                expected_version,
                ..
            } => (id.clone(), *expected_version),
            ExplicitMemoryMutation::Propose { .. } => unreachable!(),
        };
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&id)
            .filter(|record| record.scope == request.scope)
            .ok_or_else(|| MemoryError::NotFound(id.as_str().into()))?;
        if record.version != expected_version {
            return Err(MemoryError::VersionConflict {
                id: id.as_str().into(),
                expected: expected_version,
                actual: record.version,
            });
        }
        let action = match request.mutation {
            ExplicitMemoryMutation::Confirm { .. } => {
                if record.state != MemoryState::Proposed {
                    return Err(MemoryError::InvalidState {
                        id: id.as_str().into(),
                        expected: "proposed",
                        actual: record.state.as_str(),
                    });
                }
                record.state = MemoryState::Committed;
                MemoryMutationAction::Confirmed
            }
            ExplicitMemoryMutation::Update { changes, .. } => {
                changes.validate()?;
                if let Some(value) = changes.value {
                    record.value = value;
                }
                if let Some(evidence) = changes.evidence {
                    record.evidence = evidence;
                }
                if let Some(confidence) = changes.confidence {
                    record.confidence = confidence;
                }
                if let Some(sensitivity) = changes.sensitivity {
                    record.sensitivity = sensitivity;
                }
                if let Some(retention) = changes.retention {
                    record.retention = retention;
                }
                if let Some(conflict_key) = changes.conflict_key {
                    record.conflict_key = conflict_key;
                }
                MemoryMutationAction::Updated
            }
            ExplicitMemoryMutation::Forget { reason, .. } => {
                record.state = MemoryState::Tombstoned;
                record.value = MemoryValue::redacted();
                record.evidence.clear();
                record.tombstone = Some(MemoryTombstone {
                    reason,
                    at: Utc::now(),
                });
                MemoryMutationAction::Forgotten
            }
            ExplicitMemoryMutation::Propose { .. } => unreachable!(),
        };
        record.version += 1;
        record.updated_at = Utc::now();
        Ok(MemoryMutationResult {
            action,
            record: record.clone(),
            conflicts: Vec::new(),
        })
    }

    async fn export(&self, request: MemoryExportRequest) -> MemoryResult<MemoryExport> {
        self.ensure_initialized()?;
        request.scope.validate()?;
        let records = self.records.lock().unwrap();
        Ok(MemoryExport {
            schema_version: MEMORY_CONTRACT_SCHEMA_VERSION,
            scope: request.scope.clone(),
            exported_at: Utc::now(),
            records: records
                .values()
                .filter(|record| record.scope == request.scope)
                .filter(|record| request.include_proposals || record.state != MemoryState::Proposed)
                .filter(|record| {
                    request.include_tombstones || record.state != MemoryState::Tombstoned
                })
                .cloned()
                .collect(),
        })
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> MemoryResult<MemoryDeleteResult> {
        self.ensure_initialized()?;
        scope.validate()?;
        let mut records = self.records.lock().unwrap();
        let before = records.len();
        records.retain(|_, record| &record.scope != scope);
        Ok(MemoryDeleteResult {
            deleted_records: (before - records.len()) as u64,
            deleted_index_entries: 0,
        })
    }

    async fn shutdown(&self) -> MemoryResult<()> {
        self.initialized.store(false, Ordering::SeqCst);
        Ok(())
    }
}

fn committed_record(draft: MemoryDraft, scope: MemoryScope) -> MemoryRecord {
    let now = Utc::now();
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
        state: MemoryState::Committed,
        version: 1,
        conflict_key: draft.conflict_key,
        supersedes: draft.supersedes,
        superseded_by: None,
        tombstone: None,
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn fake_provider_conforms_to_lifecycle_mutation_export_and_delete_contract() {
    let provider = FakeMemoryProvider::new();
    let recall = MemoryRecallRequest {
        scope: scope(),
        query: "Chinese".into(),
        kinds: BTreeSet::new(),
        limit: 10,
    };
    assert!(provider.pre_turn_recall(recall.clone()).await.is_err());
    provider.initialize().await.unwrap();

    let proposed = provider
        .post_turn_candidates(MemoryCandidateBatch {
            scope: scope(),
            session_id: "session-1".into(),
            candidates: vec![draft("Prefers Chinese responses")],
        })
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(proposed.state, MemoryState::Proposed);
    assert!(
        provider
            .pre_turn_recall(recall.clone())
            .await
            .unwrap()
            .is_empty()
    );

    let confirmed = provider
        .mutate(MemoryMutationRequest {
            scope: scope(),
            mutation: ExplicitMemoryMutation::Confirm {
                id: proposed.id.clone(),
                expected_version: proposed.version,
            },
        })
        .await
        .unwrap()
        .record;
    assert_eq!(confirmed.state, MemoryState::Committed);
    assert_eq!(
        provider.pre_turn_recall(recall).await.unwrap(),
        vec![confirmed.clone()]
    );

    let updated = provider
        .mutate(MemoryMutationRequest {
            scope: scope(),
            mutation: ExplicitMemoryMutation::Update {
                id: confirmed.id.clone(),
                expected_version: confirmed.version,
                changes: MemoryUpdate {
                    value: Some(MemoryValue::new("Prefers concise Chinese responses").unwrap()),
                    ..MemoryUpdate::default()
                },
            },
        })
        .await
        .unwrap()
        .record;
    assert_eq!(updated.version, confirmed.version + 1);

    let stale = provider
        .mutate(MemoryMutationRequest {
            scope: scope(),
            mutation: ExplicitMemoryMutation::Forget {
                id: updated.id.clone(),
                expected_version: confirmed.version,
                reason: MemoryTombstoneReason::UserRequest,
            },
        })
        .await
        .unwrap_err();
    assert!(matches!(stale, MemoryError::VersionConflict { .. }));

    let forgotten = provider
        .mutate(MemoryMutationRequest {
            scope: scope(),
            mutation: ExplicitMemoryMutation::Forget {
                id: updated.id,
                expected_version: updated.version,
                reason: MemoryTombstoneReason::UserRequest,
            },
        })
        .await
        .unwrap()
        .record;
    forgotten.validate().unwrap();

    let exported = provider
        .export(MemoryExportRequest {
            scope: scope(),
            include_proposals: true,
            include_tombstones: true,
        })
        .await
        .unwrap();
    assert_eq!(exported.records, vec![forgotten]);
    assert_eq!(
        provider
            .delete_scope(&scope())
            .await
            .unwrap()
            .deleted_records,
        1
    );
    provider.shutdown().await.unwrap();
}
