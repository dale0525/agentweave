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

#[tokio::test]
async fn forged_cross_package_snapshot_member_preserves_the_owned_revision() {
    let fixture = AuthoringFixture::new().await;
    activate_package(&fixture, "com.example.alpha", "1.0.0").await;
    let beta = activate_package(&fixture, "com.example.beta", "1.0.0").await;
    activate_package(&fixture, "com.example.alpha", "2.0.0").await;
    let mut members: serde_json::Value =
        sqlx::query_scalar("SELECT members_json FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(fixture.state.pool())
            .await
            .map(|value: String| serde_json::from_str(&value).unwrap())
            .unwrap();
    let member = members
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|member| member["packageId"] == "com.example.alpha")
        .unwrap();
    member["revisionId"] = json!(beta);
    sqlx::query("UPDATE skill_snapshots SET members_json = ? WHERE status = 'active'")
        .bind(serde_json::to_string(&members).unwrap())
        .execute(fixture.state.pool())
        .await
        .unwrap();

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    let beta_record = fixture.state.get_revision(&beta).await.unwrap().unwrap();
    assert_eq!(beta_record.status, SkillRevisionStatus::Managed);
    assert!(std::path::Path::new(&beta_record.storage_path).is_dir());
    assert_eq!(
        fixture
            .state
            .get_installation(&SkillPackageId::parse("com.example.beta").unwrap())
            .await
            .unwrap()
            .unwrap()
            .active_revision_id
            .as_deref(),
        Some(beta.as_str())
    );
    let diagnostics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation = 'snapshot_member_ownership_mismatch'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics, 1);
}

#[tokio::test]
async fn startup_enumeration_uses_one_global_budget_across_packages() {
    let fixture = AuthoringFixture::with_limits(crate::skill_store::SkillStoreLimits {
        max_directories: 4,
        ..crate::skill_store::SkillStoreLimits::default()
    })
    .await;
    for package in ["com.example.alpha", "com.example.beta"] {
        let revisions = fixture
            .store
            .paths()
            .managed
            .join(package)
            .join("revisions");
        tokio::fs::create_dir_all(&revisions).await.unwrap();
        for suffix in ["one", "two"] {
            tokio::fs::create_dir(revisions.join(suffix)).await.unwrap();
        }
    }

    fixture.manager.startup_reconcile().await.unwrap();

    let diagnostics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation = 'startup_enumeration_limit_exceeded'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics, 1);
    assert!(
        fixture
            .store
            .paths()
            .managed
            .join("com.example.alpha")
            .is_dir()
    );
    assert!(
        fixture
            .store
            .paths()
            .managed
            .join("com.example.beta")
            .is_dir()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_startup_entry_kinds_are_preserved_and_diagnosed_idempotently() {
    use std::os::unix::fs::symlink;

    let fixture = AuthoringFixture::new().await;
    let symlink_path = fixture.store.paths().staging.join("unknown-link");
    symlink(fixture.store.paths().managed.clone(), &symlink_path).unwrap();
    let regular_path = fixture.store.paths().quarantine.join("unknown-file");
    tokio::fs::write(&regular_path, b"evidence").await.unwrap();
    let orphan_package = fixture.store.paths().managed.join("com.example.orphan");
    tokio::fs::create_dir(&orphan_package).await.unwrap();

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    assert!(tokio::fs::symlink_metadata(&symlink_path).await.is_ok());
    assert!(regular_path.is_file());
    assert!(orphan_package.is_dir());
    let diagnostics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation = 'unknown_startup_entry'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics, 3);
    let leaked_paths: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation = 'unknown_startup_entry' AND metadata_json LIKE ?",
    )
    .bind(format!("%{}%", fixture.store.paths().managed.display()))
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(leaked_paths, 0);
}

#[tokio::test]
async fn invalid_last_known_good_records_one_generation_phase_diagnostic() {
    let fixture = AuthoringFixture::new().await;
    activate_package(&fixture, "com.example.alpha", "1.0.0").await;
    activate_package(&fixture, "com.example.alpha", "2.0.0").await;
    let generation: i64 = sqlx::query_scalar(
        "SELECT generation FROM skill_snapshots WHERE status = 'last_known_good'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE skill_snapshots SET members_json = '{\"invalid\":true}' WHERE status = 'last_known_good'",
    )
    .execute(fixture.state.pool())
    .await
    .unwrap();

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    let diagnostics: Vec<(String, String)> = sqlx::query_as(
        "SELECT idempotency_key, metadata_json FROM skill_maintenance_diagnostics WHERE operation = 'invalid_last_known_good_snapshot'",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].0.contains(&generation.to_string()));
    let metadata: serde_json::Value = serde_json::from_str(&diagnostics[0].1).unwrap();
    assert_eq!(metadata["generation"], generation);
    assert_eq!(metadata["phase"], "rebuild");
    assert!(!diagnostics[0].1.contains("/"));
}

#[tokio::test]
async fn row_only_managed_and_quarantined_records_are_preserved_and_diagnosed() {
    let fixture = AuthoringFixture::new().await;
    let managed = activate_package(&fixture, "com.example.managed-row", "1.0.0").await;
    let managed_record = fixture.state.get_revision(&managed).await.unwrap().unwrap();
    make_tree_writable_for_test(std::path::Path::new(&managed_record.storage_path));
    tokio::fs::remove_dir_all(&managed_record.storage_path)
        .await
        .unwrap();
    let draft = fixture.draft().await;
    let quarantined = fixture
        .store
        .quarantine_revision(&draft.revision_id, "test quarantine")
        .await
        .unwrap();
    make_tree_writable_for_test(&quarantined.path);
    tokio::fs::remove_dir_all(&quarantined.path).await.unwrap();

    fixture.manager.startup_reconcile().await.unwrap();

    assert_eq!(
        fixture
            .state
            .get_revision(&managed)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Managed
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Quarantined
    );
    let operations: Vec<String> = sqlx::query_scalar(
        "SELECT operation FROM skill_maintenance_diagnostics WHERE operation IN ('row_only_managed', 'row_only_quarantine') ORDER BY operation",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(operations, ["row_only_managed", "row_only_quarantine"]);
}

#[tokio::test]
async fn process_local_store_issue_is_carried_into_durable_startup_diagnostics() {
    let faults = crate::skill_store::SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = fixture.draft().await;
    faults.fail_once(crate::skill_store::SkillStoreFaultPoint::PromoteSourceCleanupAfter);
    fixture
        .store
        .promote_revision(&draft.revision_id)
        .await
        .unwrap();
    assert_eq!(fixture.store.maintenance_issues().len(), 1);

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    let diagnostics: Vec<String> = sqlx::query_scalar(
        "SELECT metadata_json FROM skill_maintenance_diagnostics WHERE area = 'store' AND metadata_json LIKE '%process_local_carryover%'",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert!(!diagnostics[0].contains(fixture.store.paths().staging.to_string_lossy().as_ref()));
}

pub(crate) async fn activate_package(
    fixture: &AuthoringFixture,
    package: &str,
    version: &str,
) -> String {
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

fn make_tree_writable_for_test(root: &std::path::Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(if metadata.is_dir() { 0o700 } else { 0o600 });
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        if metadata.is_dir() {
            stack.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
}
