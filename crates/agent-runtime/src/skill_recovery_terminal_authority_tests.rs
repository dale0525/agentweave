use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_policy::{SkillGrant, SkillManagementPolicy};
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::{ManagedSkillSource, SkillSource};
use crate::skill_state::{SkillSnapshotStatus, SkillStateBoundaryError};
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use std::sync::Arc;

#[tokio::test]
async fn startup_verifies_lkg_before_the_first_managed_mutation() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::RecoveryBeforeFirstManagedMutation);
    let fixture = AuthoringFixture::with_faults(faults).await;
    activate_new_revision(&fixture, "1.0.0").await;
    let active_revision = activate_new_revision(&fixture, "2.0.0").await;
    let lkg = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::LastKnownGood)
        .await
        .unwrap()
        .unwrap();
    let record = fixture
        .state
        .get_revision(&active_revision)
        .await
        .unwrap()
        .unwrap();
    let descriptor = std::path::Path::new(&record.storage_path).join("general-agent.json");
    make_file_writable(&descriptor).await;
    tokio::fs::write(&descriptor, b"{}\n").await.unwrap();
    let restarted = manager_for_store(&fixture).await;
    let _service = bind_manager(&fixture, restarted.clone());
    let reconcile_manager = restarted.clone();
    let reconcile = tokio::spawn(async move { reconcile_manager.startup_reconcile().await });

    gate.wait_entered().await;
    assert_eq!(restarted.current_snapshot().generation(), lkg.generation);
    assert_eq!(
        fixture
            .state
            .get_revision(&active_revision)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::skill_state::SkillRevisionStatus::Managed
    );
    gate.release().await;
    reconcile.await.unwrap().unwrap();
}

#[tokio::test]
async fn startup_without_any_verified_authority_preserves_all_managed_state() {
    let fixture = AuthoringFixture::new().await;
    let active_revision = activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap();
    fixture.draft().await;
    sqlx::query(
        "UPDATE skill_snapshots SET members_json = '[{\"packageId\":\"invalid\"}]' WHERE status = 'last_known_good'",
    )
    .execute(fixture.state.pool())
    .await
    .unwrap();
    let record = fixture
        .state
        .get_revision(&active_revision)
        .await
        .unwrap()
        .unwrap();
    let descriptor = std::path::Path::new(&record.storage_path).join("general-agent.json");
    make_file_writable(&descriptor).await;
    tokio::fs::write(&descriptor, b"{}\n").await.unwrap();
    let before = startup_managed_fingerprint(&fixture).await;
    let restarted = manager_for_store(&fixture).await;
    let _service = bind_manager(&fixture, restarted.clone());

    let error = restarted.startup_reconcile().await.unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    assert_eq!(
        error.to_string(),
        "skill state conflicts with current state"
    );
    assert_eq!(startup_managed_fingerprint(&fixture).await, before);
}

fn bind_manager(fixture: &AuthoringFixture, manager: SkillManager) -> OwnerSkillManagementService {
    OwnerSkillManagementService::new(
        manager,
        fixture.store.clone(),
        fixture.state.clone(),
        SkillManagementPolicy::owner_only(),
    )
}

async fn manager_for_store(fixture: &AuthoringFixture) -> SkillManager {
    SkillManager::new_deferred_managed(SkillManagerConfig {
        sources: vec![
            Arc::new(ManagedSkillSource::from_store(fixture.store.clone())) as Arc<dyn SkillSource>,
        ],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct StartupManagedFingerprint {
    revisions: Vec<(String, String, String)>,
    approvals: Vec<(String, String, String)>,
    diagnostics: Vec<(String, String, String)>,
    trees: Vec<String>,
}

async fn startup_managed_fingerprint(fixture: &AuthoringFixture) -> StartupManagedFingerprint {
    let revisions = sqlx::query_as(
        "SELECT revision_id, lifecycle_status, storage_path FROM skill_revisions ORDER BY revision_id",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    let approvals = sqlx::query_as(
        "SELECT approval_id, status, requested_by FROM skill_approvals ORDER BY approval_id",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    let diagnostics = sqlx::query_as(
        "SELECT idempotency_key, operation, outcome FROM skill_maintenance_diagnostics ORDER BY idempotency_key",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    let mut trees = Vec::new();
    for (area, root) in [
        ("managed", &fixture.store.paths().managed),
        ("staging", &fixture.store.paths().staging),
        ("quarantine", &fixture.store.paths().quarantine),
    ] {
        let mut entries = tokio::fs::read_dir(root).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name != ".locks" {
                trees.push(format!("{area}:{name}"));
            }
        }
    }
    trees.sort();
    StartupManagedFingerprint {
        revisions,
        approvals,
        diagnostics,
        trees,
    }
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
