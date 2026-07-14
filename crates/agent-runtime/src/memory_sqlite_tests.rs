use super::*;
use crate::memory::{
    MemoryConfidence, MemoryEvidence, MemoryEvidenceSource, MemoryKind, MemoryRetention,
    MemorySensitivity,
};
use chrono::Duration as ChronoDuration;
use std::collections::BTreeSet;
use tempfile::TempDir;

struct TestDatabase {
    provider: SqliteMemoryProvider,
    _directory: TempDir,
}

async fn database() -> TestDatabase {
    let directory = TempDir::new().expect("create temporary directory");
    let path = directory.path().join("memory.sqlite");
    let provider = SqliteMemoryProvider::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("connect memory database");
    TestDatabase {
        provider,
        _directory: directory,
    }
}

fn scope(app: &str, tenant: &str, user: &str) -> MemoryScope {
    MemoryScope::new(app, tenant, user).expect("valid scope")
}

fn draft(text: &str, conflict_key: Option<&str>) -> MemoryDraft {
    MemoryDraft {
        kind: MemoryKind::parse(MemoryKind::PREFERENCE).expect("valid kind"),
        value: MemoryValue::new(text).expect("valid value"),
        evidence: vec![MemoryEvidence {
            source: MemoryEvidenceSource::UserStatement,
            source_id: Some("turn-1".into()),
            excerpt: Some(text.into()),
            observed_at: Utc::now(),
        }],
        confidence: MemoryConfidence::from_basis_points(8_500).expect("valid confidence"),
        sensitivity: MemorySensitivity::Personal,
        retention: MemoryRetention::Persistent,
        conflict_key: conflict_key.map(str::to_owned),
        supersedes: None,
    }
}

async fn propose(
    provider: &SqliteMemoryProvider,
    scope: &MemoryScope,
    draft: MemoryDraft,
) -> MemoryMutationResult {
    provider
        .mutate(MemoryMutationRequest {
            scope: scope.clone(),
            mutation: ExplicitMemoryMutation::Propose { draft },
        })
        .await
        .expect("propose memory")
}

async fn confirm(
    provider: &SqliteMemoryProvider,
    scope: &MemoryScope,
    record: &MemoryRecord,
) -> MemoryMutationResult {
    provider
        .mutate(MemoryMutationRequest {
            scope: scope.clone(),
            mutation: ExplicitMemoryMutation::Confirm {
                id: record.id.clone(),
                expected_version: record.version,
            },
        })
        .await
        .expect("confirm memory")
}

async fn committed(
    provider: &SqliteMemoryProvider,
    scope: &MemoryScope,
    draft: MemoryDraft,
) -> MemoryMutationResult {
    let proposal = propose(provider, scope, draft).await;
    confirm(provider, scope, &proposal.record).await
}

async fn recall(
    provider: &SqliteMemoryProvider,
    scope: &MemoryScope,
    query: &str,
) -> Vec<MemoryRecord> {
    provider
        .pre_turn_recall(MemoryRecallRequest {
            scope: scope.clone(),
            query: query.into(),
            kinds: BTreeSet::new(),
            limit: 20,
        })
        .await
        .expect("recall memory")
}

#[tokio::test]
async fn migrates_schema_once_and_reports_the_supported_version() {
    let database = database().await;
    database
        .provider
        .initialize()
        .await
        .expect("idempotent migration");

    let version: i64 =
        sqlx::query_scalar("SELECT version FROM memory_schema_meta WHERE singleton = 1")
            .fetch_one(database.provider.pool())
            .await
            .expect("read schema version");
    let indexes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'memory_records_%'",
    )
    .fetch_one(database.provider.pool())
    .await
    .expect("read schema indexes");

    assert_eq!(version, MEMORY_SQLITE_SCHEMA_VERSION);
    assert_eq!(indexes, 4);
}

#[tokio::test]
async fn isolates_every_scope_component() {
    let database = database().await;
    let scopes = [
        scope("assistant", "tenant-a", "user-a"),
        scope("other-app", "tenant-a", "user-a"),
        scope("assistant", "tenant-b", "user-a"),
        scope("assistant", "tenant-a", "user-b"),
    ];
    for (index, item_scope) in scopes.iter().enumerate() {
        committed(
            &database.provider,
            item_scope,
            draft(&format!("private marker {index}"), None),
        )
        .await;
    }

    for (index, item_scope) in scopes.iter().enumerate() {
        let records = recall(&database.provider, item_scope, "private marker").await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].value.text, format!("private marker {index}"));
    }
}

#[tokio::test]
async fn proposals_are_not_recalled_until_explicitly_confirmed() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let proposal = propose(&database.provider, &scope, draft("quiet workspace", None)).await;

    assert_eq!(proposal.action, MemoryMutationAction::Proposed);
    assert!(
        recall(&database.provider, &scope, "workspace")
            .await
            .is_empty()
    );
    assert_eq!(
        database
            .provider
            .get(&scope, &proposal.record.id, false)
            .await
            .expect("get proposal")
            .expect("proposal exists")
            .state,
        MemoryState::Proposed
    );

    let confirmation = confirm(&database.provider, &scope, &proposal.record).await;
    assert_eq!(confirmation.action, MemoryMutationAction::Confirmed);
    assert_eq!(confirmation.record.version, 2);
    assert_eq!(
        recall(&database.provider, &scope, "workspace").await.len(),
        1
    );
}

#[tokio::test]
async fn search_handles_unicode_casefold_and_cjk_substrings() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    committed(
        &database.provider,
        &scope,
        draft("I like STRASSE walks and 北京豆汁", None),
    )
    .await;

    assert_eq!(recall(&database.provider, &scope, "Straße").await.len(), 1);
    assert_eq!(recall(&database.provider, &scope, "北京").await.len(), 1);
    assert_eq!(recall(&database.provider, &scope, "上海").await.len(), 0);
}

#[tokio::test]
async fn reports_conflicts_without_resolving_them_implicitly() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let first = committed(
        &database.provider,
        &scope,
        draft("prefers tea", Some("drink")),
    )
    .await;
    let second = committed(
        &database.provider,
        &scope,
        draft("prefers coffee", Some("drink")),
    )
    .await;

    assert_eq!(second.conflicts, vec![first.record.id]);
    assert_eq!(recall(&database.provider, &scope, "prefers").await.len(), 2);
}

#[tokio::test]
async fn explicit_supersession_hides_the_replaced_record() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let old = committed(
        &database.provider,
        &scope,
        draft("prefers tea", Some("drink")),
    )
    .await;
    let mut replacement = draft("prefers coffee", Some("drink"));
    replacement.supersedes = Some(old.record.id.clone());
    let new = committed(&database.provider, &scope, replacement).await;

    assert!(new.conflicts.is_empty());
    let recalled = recall(&database.provider, &scope, "prefers").await;
    assert_eq!(recalled.len(), 1);
    assert_eq!(recalled[0].id, new.record.id);
    let replaced = database
        .provider
        .get(&scope, &old.record.id, false)
        .await
        .expect("get replaced memory")
        .expect("replaced memory exists");
    assert_eq!(replaced.superseded_by, Some(new.record.id));
    assert_eq!(replaced.version, old.record.version + 1);
}

#[tokio::test]
async fn update_uses_optimistic_compare_and_swap() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let current = committed(&database.provider, &scope, draft("old value", None)).await;
    let request = |value: &str| MemoryMutationRequest {
        scope: scope.clone(),
        mutation: ExplicitMemoryMutation::Update {
            id: current.record.id.clone(),
            expected_version: current.record.version,
            changes: MemoryUpdate {
                value: Some(MemoryValue::new(value).expect("valid value")),
                ..MemoryUpdate::default()
            },
        },
    };

    let (left, right) = tokio::join!(
        database.provider.mutate(request("left winner")),
        database.provider.mutate(request("right winner")),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let error = left.err().or_else(|| right.err()).expect("one CAS error");
    assert!(matches!(error, MemoryError::VersionConflict { .. }));

    let stored = database
        .provider
        .get(&scope, &current.record.id, false)
        .await
        .expect("get updated memory")
        .expect("updated memory exists");
    assert_eq!(stored.version, current.record.version + 1);
}

#[tokio::test]
async fn expiry_and_session_end_create_scrubbed_tombstones() {
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let mut expiring = draft("short lived secret", None);
    expiring.retention = MemoryRetention::ExpiresAt {
        expires_at: Utc::now() + ChronoDuration::milliseconds(40),
    };
    let expiring = committed(&database.provider, &scope, expiring).await;
    std::thread::sleep(std::time::Duration::from_millis(60));
    assert!(
        recall(&database.provider, &scope, "secret")
            .await
            .is_empty()
    );
    let expired = database
        .provider
        .get(&scope, &expiring.record.id, true)
        .await
        .expect("get expired memory")
        .expect("expired tombstone exists");
    assert_eq!(expired.state, MemoryState::Tombstoned);
    assert_eq!(
        expired.tombstone.expect("expiry tombstone").reason,
        MemoryTombstoneReason::Expired
    );
    assert!(expired.value.text.is_empty());
    assert!(expired.evidence.is_empty());

    let mut session_draft = draft("session only secret", None);
    session_draft.retention = MemoryRetention::Session {
        session_id: "session-1".into(),
    };
    let session_records = database
        .provider
        .on_session_end(MemoryCandidateBatch {
            scope: scope.clone(),
            session_id: "session-1".into(),
            candidates: vec![session_draft],
        })
        .await
        .expect("end session");
    let ended = database
        .provider
        .get(&scope, &session_records[0].id, true)
        .await
        .expect("get ended session memory")
        .expect("session tombstone exists");
    assert_eq!(ended.state, MemoryState::Tombstoned);
    assert_eq!(
        ended.tombstone.expect("session tombstone").reason,
        MemoryTombstoneReason::SessionEnded
    );
}

#[tokio::test]
async fn forget_scrubs_source_data_and_removes_the_derived_index() {
    const SECRET: &str = "SECRET-MARKER-7139";
    let database = database().await;
    let scope = scope("assistant", "tenant", "user");
    let current = committed(&database.provider, &scope, draft(SECRET, None)).await;

    let forgotten = database
        .provider
        .mutate(MemoryMutationRequest {
            scope: scope.clone(),
            mutation: ExplicitMemoryMutation::Forget {
                id: current.record.id.clone(),
                expected_version: current.record.version,
                reason: MemoryTombstoneReason::UserRequest,
            },
        })
        .await
        .expect("forget memory");

    assert_eq!(forgotten.action, MemoryMutationAction::Forgotten);
    assert!(forgotten.record.value.text.is_empty());
    assert!(forgotten.record.evidence.is_empty());
    assert!(
        database
            .provider
            .get(&scope, &current.record.id, false)
            .await
            .expect("get hidden tombstone")
            .is_none()
    );
    let raw: (String, String) = sqlx::query_as(
        "SELECT value_json, evidence_json FROM memory_records WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND id = ?",
    )
    .bind(&scope.app_id)
    .bind(&scope.tenant_id)
    .bind(&scope.user_id)
    .bind(current.record.id.as_str())
    .fetch_one(database.provider.pool())
    .await
    .expect("read scrubbed row");
    assert!(!raw.0.contains(SECRET));
    assert_eq!(raw.1, "[]");
    let index_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_search WHERE app_id = ? AND tenant_id = ? AND user_id = ? AND memory_id = ?",
    )
    .bind(&scope.app_id)
    .bind(&scope.tenant_id)
    .bind(&scope.user_id)
    .bind(current.record.id.as_str())
    .fetch_one(database.provider.pool())
    .await
    .expect("count index rows");
    assert_eq!(index_count, 0);
}

#[tokio::test]
async fn export_flags_and_scope_deletion_are_precise() {
    let database = database().await;
    let target = scope("assistant", "tenant", "target-user");
    let neighbor = scope("assistant", "tenant", "neighbor-user");
    let proposal = propose(&database.provider, &target, draft("proposal", None)).await;
    let live = committed(&database.provider, &target, draft("live", None)).await;
    committed(&database.provider, &neighbor, draft("neighbor", None)).await;
    database
        .provider
        .mutate(MemoryMutationRequest {
            scope: target.clone(),
            mutation: ExplicitMemoryMutation::Forget {
                id: live.record.id,
                expected_version: live.record.version,
                reason: MemoryTombstoneReason::UserRequest,
            },
        })
        .await
        .expect("create tombstone");

    let default_export = database
        .provider
        .export(MemoryExportRequest {
            scope: target.clone(),
            include_proposals: false,
            include_tombstones: false,
        })
        .await
        .expect("default export");
    assert!(default_export.records.is_empty());
    let full_export = database
        .provider
        .export(MemoryExportRequest {
            scope: target.clone(),
            include_proposals: true,
            include_tombstones: true,
        })
        .await
        .expect("full export");
    assert_eq!(full_export.records.len(), 2);
    assert!(
        full_export
            .records
            .iter()
            .any(|record| record.id == proposal.record.id)
    );

    let deleted = database
        .provider
        .delete_scope(&target)
        .await
        .expect("delete target scope");
    assert_eq!(deleted.deleted_records, 2);
    assert_eq!(deleted.deleted_index_entries, 0);
    assert!(
        database
            .provider
            .export(MemoryExportRequest {
                scope: target,
                include_proposals: true,
                include_tombstones: true,
            })
            .await
            .expect("export deleted scope")
            .records
            .is_empty()
    );
    assert_eq!(
        recall(&database.provider, &neighbor, "neighbor")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn debug_and_errors_do_not_expose_memory_payloads_or_scope_principals() {
    const SECRET: &str = "SECRET-MARKER-9841";
    let database = database().await;
    let scope = scope("assistant", "private-tenant", "private-user");
    let draft = draft(SECRET, Some("private-conflict-key"));
    let debug = format!("{scope:?} {draft:?}");
    assert!(!debug.contains(SECRET));
    assert!(!debug.contains("private-tenant"));
    assert!(!debug.contains("private-user"));
    assert!(!debug.contains("private-conflict-key"));

    let proposal = propose(&database.provider, &scope, draft).await;
    let error = database
        .provider
        .mutate(MemoryMutationRequest {
            scope,
            mutation: ExplicitMemoryMutation::Confirm {
                id: proposal.record.id,
                expected_version: proposal.record.version + 1,
            },
        })
        .await
        .expect_err("stale version must fail");
    let error_text = format!("{error:?} {error}");
    assert!(!error_text.contains(SECRET));
    assert!(!error_text.contains("private-tenant"));
    assert!(!error_text.contains("private-user"));
}
