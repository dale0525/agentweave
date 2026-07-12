use crate::skill_authoring_tests::{AuthoringFixture, update};
use crate::skill_management::CreateSkillDraftRequest;
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_state::{SkillApprovalStatus, SkillRevisionStatus};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn tree_only_staging_and_quarantine_are_preserved_and_reported_once() {
    let fixture = AuthoringFixture::new().await;
    let staging_id = uuid::Uuid::new_v4().to_string();
    let quarantine_id = uuid::Uuid::new_v4().to_string();
    let staging = fixture.store.paths().staging.join(&staging_id);
    let quarantine = fixture.store.paths().quarantine.join(&quarantine_id);
    tokio::fs::create_dir(&staging).await.unwrap();
    tokio::fs::create_dir(&quarantine).await.unwrap();

    let first = fixture.manager.startup_reconcile().await.unwrap();
    let second = fixture.manager.startup_reconcile().await.unwrap();

    assert!(staging.is_dir());
    assert!(quarantine.is_dir());
    assert!(first.maintenance_diagnostics >= 2);
    assert!(second.maintenance_diagnostics >= 2);
    let diagnostics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation IN ('tree_only_staging', 'tree_only_quarantine')",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics, 2);
}

#[tokio::test]
async fn row_only_staging_revision_is_removed_by_exact_state_cas() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    tokio::fs::remove_dir_all(&record.storage_path)
        .await
        .unwrap();

    fixture.manager.startup_reconcile().await.unwrap();

    assert!(
        fixture
            .state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn incomplete_promotion_destination_is_removed_only_after_binding_verification() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let destination = fixture
        .store
        .paths()
        .managed
        .join(record.package_id.as_str())
        .join("revisions")
        .join(&record.revision_id);
    tokio::fs::create_dir_all(&destination).await.unwrap();
    let mut entries = tokio::fs::read_dir(&record.storage_path).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry.file_type().await.unwrap().is_file() {
            tokio::fs::copy(entry.path(), destination.join(entry.file_name()))
                .await
                .unwrap();
        }
    }

    fixture.manager.startup_reconcile().await.unwrap();

    assert!(!destination.exists());
    let retained = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, SkillRevisionStatus::Staging);
    assert!(std::path::Path::new(&retained.storage_path).is_dir());
}

#[tokio::test]
async fn stale_pending_activation_approval_is_terminal_and_idempotent() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![update(
                "SKILL.md",
                "---\nname: changed\ndescription: changed\n---\n",
            )],
        )
        .await
        .unwrap();

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    let resolved = fixture
        .state
        .get_approval(&approval.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.status, SkillApprovalStatus::Rejected);
    assert_eq!(resolved.approved_by.as_deref(), Some("system-recovery"));
}

#[tokio::test]
async fn invalid_package_recovery_preserves_an_unrelated_active_package() {
    let fixture = AuthoringFixture::new().await;
    activate_package(&fixture, "com.example.alpha", "1.0.0").await;
    let beta = activate_package(&fixture, "com.example.beta", "1.0.0").await;
    let broken = activate_package(&fixture, "com.example.alpha", "2.0.0").await;
    corrupt_descriptor(&fixture, &broken).await;

    fixture.manager.startup_reconcile().await.unwrap();

    let snapshot = fixture.manager.current_snapshot();
    assert!(snapshot.packages().iter().any(|resolved| {
        resolved.package.descriptor.id.as_str() == "com.example.beta"
            && resolved
                .package
                .verified_content
                .as_ref()
                .and_then(|content| content.execution_binding.as_ref())
                .is_some_and(|binding| binding.revision_id == beta)
    }));
}

#[tokio::test]
async fn durable_snapshot_failure_keeps_the_previous_memory_snapshot() {
    let fixture = AuthoringFixture::new().await;
    let previous = fixture.manager.current_snapshot();
    sqlx::query("DROP TABLE skill_snapshots")
        .execute(fixture.state.pool())
        .await
        .unwrap();

    fixture.manager.startup_reconcile().await.unwrap_err();

    assert!(Arc::ptr_eq(&previous, &fixture.manager.current_snapshot()));
}

async fn activate_package(fixture: &AuthoringFixture, package: &str, version: &str) -> String {
    let draft = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse(package).unwrap(),
                display_name: package.into(),
                description: format!("{package} instructions"),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let mut descriptor = record.descriptor_json;
    descriptor["version"] = json!(version);
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![update(
                "general-agent.json",
                format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
            )],
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap();
    draft.revision_id
}

async fn corrupt_descriptor(fixture: &AuthoringFixture, revision_id: &str) {
    let record = fixture
        .state
        .get_revision(revision_id)
        .await
        .unwrap()
        .unwrap();
    let path = std::path::Path::new(&record.storage_path).join("general-agent.json");
    make_file_writable(&path).await;
    tokio::fs::write(path, b"corrupt").await.unwrap();
}

async fn make_file_writable(path: &std::path::Path) {
    let mut permissions = tokio::fs::metadata(path).await.unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await.unwrap();
}
