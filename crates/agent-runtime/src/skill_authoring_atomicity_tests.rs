use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService,
};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use crate::skill_source::ManagedSkillSource;
use crate::skill_state::SkillStateStore;
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

async fn fixture(
    faults: SkillStoreTestFaults,
) -> (
    OwnerSkillManagementService,
    SkillStateStore,
    crate::skill_management::SkillDraftSummary,
    SkillManager,
) {
    fixture_with_policy(faults, SkillManagementPolicy::owner_only()).await
}

async fn fixture_with_policy(
    faults: SkillStoreTestFaults,
    policy: SkillManagementPolicy,
) -> (
    OwnerSkillManagementService,
    SkillStateStore,
    crate::skill_management::SkillDraftSummary,
    SkillManager,
) {
    let app = tempdir().unwrap().keep();
    let cache = tempdir().unwrap().keep();
    let paths = SkillStorePaths::prepare(&app, &cache).await.unwrap();
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let store = SkillRevisionStore::with_test_faults(
        paths,
        state.clone(),
        SkillStoreLimits::default(),
        faults,
    );
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(store.clone()))],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: policy.protected_packages.iter().cloned().collect(),
        allowed_overrides: policy.allowed_overrides.iter().cloned().collect(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap();
    let service = OwnerSkillManagementService::new(manager.clone(), store, state.clone(), policy);
    let draft = service
        .create_draft(
            &ActorContext::owner("owner-1", [SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.atomic").unwrap(),
                display_name: "Atomic".into(),
                description: "Atomic updates.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    (service, state, draft, manager)
}

#[tokio::test]
async fn disallowed_kind_change_preserves_original_tree_and_row() {
    let mut policy = SkillManagementPolicy::owner_only();
    policy.allowed_kinds = [SkillPackageKind::InstructionOnly].into_iter().collect();
    let (service, state, draft, _) =
        fixture_with_policy(SkillStoreTestFaults::default(), policy).await;
    let before = state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let mut descriptor = before.descriptor_json.clone();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["runtimeTools"] = serde_json::json!(["calendar_read"]);

    let error = service
        .update_draft(
            &ActorContext::owner("owner-1", [SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![DraftFileUpdate {
                path: PathBuf::from("general-agent.json"),
                content: format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
            }],
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("kind cannot be changed"));
    assert_eq!(
        state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
}

fn updates() -> Vec<DraftFileUpdate> {
    vec![
        DraftFileUpdate {
            path: PathBuf::from("references/one.md"),
            content: "one".into(),
        },
        DraftFileUpdate {
            path: PathBuf::from("references/two.md"),
            content: "two".into(),
        },
    ]
}

#[tokio::test]
async fn second_file_failure_preserves_original_tree_and_row() {
    let faults = SkillStoreTestFaults::default();
    faults.fail_after(SkillStoreFaultPoint::WriteBeforeRename, 1);
    let (service, state, draft, _) = fixture(faults).await;
    let before = state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();

    service
        .update_draft(
            &ActorContext::owner("owner-1", [SkillGrant::EditDraft]),
            &draft.revision_id,
            updates(),
        )
        .await
        .unwrap_err();

    let after = state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    let root = PathBuf::from(after.storage_path);
    assert!(!root.join("references/one.md").exists());
    assert!(!root.join("references/two.md").exists());
}

#[tokio::test]
async fn outer_cancellation_finishes_one_consistent_multi_file_commit() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::WriteBeforeMetadataCommit);
    let (service, state, draft, _) = fixture(faults).await;
    let before = state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let task = tokio::spawn({
        let service = service.clone();
        let revision_id = draft.revision_id.clone();
        async move {
            service
                .update_draft(
                    &ActorContext::owner("owner-1", [SkillGrant::EditDraft]),
                    &revision_id,
                    updates(),
                )
                .await
        }
    });
    gate.wait_entered().await;
    task.abort();
    gate.release().await;

    let after = loop {
        let record = state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap();
        if record.content_hash != before.content_hash {
            break record;
        }
        tokio::task::yield_now().await;
    };
    let root = PathBuf::from(after.storage_path);
    assert_eq!(
        tokio::fs::read_to_string(root.join("references/one.md"))
            .await
            .unwrap(),
        "one"
    );
    assert_eq!(
        tokio::fs::read_to_string(root.join("references/two.md"))
            .await
            .unwrap(),
        "two"
    );
}

#[tokio::test]
async fn approval_audit_failure_does_not_leave_a_pending_request() {
    let (service, state, draft, _) = fixture(SkillStoreTestFaults::default()).await;
    service
        .validate_draft(
            &ActorContext::owner("validator", [SkillGrant::Validate]),
            &draft.revision_id,
        )
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_approval_audit BEFORE INSERT ON skill_audit_log
           WHEN NEW.operation = 'skill_approval_required'
           BEGIN SELECT RAISE(FAIL, 'approval audit failure'); END"#,
    )
    .execute(state.pool())
    .await
    .unwrap();

    service
        .request_activation(
            &ActorContext::owner("requester", [SkillGrant::Activate]),
            &draft.revision_id,
        )
        .await
        .unwrap_err();

    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_approvals WHERE revision_id = ? AND status = 'pending'",
    )
    .bind(&draft.revision_id)
    .fetch_one(state.pool())
    .await
    .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn publication_audit_failure_keeps_the_old_snapshot_and_installation() {
    let (service, state, draft, manager) = fixture(SkillStoreTestFaults::default()).await;
    service
        .validate_draft(
            &ActorContext::owner("validator", [SkillGrant::Validate]),
            &draft.revision_id,
        )
        .await
        .unwrap();
    let approval = service
        .request_activation(
            &ActorContext::owner("requester", [SkillGrant::Activate]),
            &draft.revision_id,
        )
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_publication_audit BEFORE INSERT ON skill_audit_log
           WHEN NEW.operation = 'skill_snapshot_published'
           BEGIN SELECT RAISE(FAIL, 'publication audit failure'); END"#,
    )
    .execute(state.pool())
    .await
    .unwrap();

    service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver", [SkillGrant::Activate]),
        )
        .await
        .unwrap_err();

    assert_eq!(manager.current_snapshot().generation(), 1);
    assert!(
        state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_none()
    );
}
