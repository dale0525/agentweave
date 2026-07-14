use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::SkillRollbackOutcome;
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::SkillLayer;
use std::process::{Command, Stdio};

#[tokio::test]
async fn independent_manager_converges_before_turn_across_every_lifecycle_publication() {
    let fixture = AuthoringFixture::with_faults(Default::default()).await;
    let (second, _, _) = fixture.second_runtime().await;
    second.startup_reconcile().await.unwrap();
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();

    let first = activate_new_revision(&fixture, "1.0.0").await;
    assert_active_revision(&second.lease_snapshot_for_turn().await.unwrap(), &first);

    let second_revision = activate_new_revision(&fixture, "2.0.0").await;
    assert_active_revision(
        &second.lease_snapshot_for_turn().await.unwrap(),
        &second_revision,
    );

    let rollback = fixture
        .service
        .rollback_managed_skill(&fixture.actor([SkillGrant::Rollback]), &package_id, &first)
        .await
        .unwrap();
    assert!(matches!(rollback, SkillRollbackOutcome::Published(_)));
    assert_active_revision(&second.lease_snapshot_for_turn().await.unwrap(), &first);

    fixture
        .service
        .disable_managed_skill(&fixture.actor([SkillGrant::Disable]), &package_id)
        .await
        .unwrap();
    assert!(
        second
            .lease_snapshot_for_turn()
            .await
            .unwrap()
            .snapshot()
            .packages()
            .is_empty()
    );

    let approval = fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_removal(
            &approval.approval_id,
            &ActorContext::owner("independent-approver", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap();
    let removed = second.lease_snapshot_for_turn().await.unwrap();
    assert!(removed.snapshot().packages().is_empty());
    assert_eq!(
        removed.generation(),
        fixture.manager.current_snapshot().generation()
    );
}

#[tokio::test]
async fn cleanup_honors_another_manager_durable_turn_lease_until_release() {
    let fixture = AuthoringFixture::with_faults(Default::default()).await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    let first_record = fixture.state.get_revision(&first).await.unwrap().unwrap();
    let (second, _, _) = fixture.second_runtime().await;
    second.startup_reconcile().await.unwrap();
    let old_turn = second.lease_snapshot_for_turn().await.unwrap();
    assert_active_revision(&old_turn, &first);

    activate_new_revision(&fixture, "2.0.0").await;
    activate_new_revision(&fixture, "3.0.0").await;
    let retained = fixture
        .manager
        .cleanup_unreferenced_revisions()
        .await
        .unwrap();
    assert!(retained.retained_revisions.contains(&first));
    assert!(std::path::Path::new(&first_record.storage_path).is_dir());

    drop(old_turn);
    for _ in 0..100 {
        let cleanup = fixture
            .manager
            .cleanup_unreferenced_revisions()
            .await
            .unwrap();
        if cleanup.deleted_revisions.contains(&first) {
            assert!(!std::path::Path::new(&first_record.storage_path).exists());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("released durable snapshot lease did not unblock revision cleanup");
}

#[tokio::test]
async fn subprocess_crash_protects_old_revision_until_durable_lease_expires() {
    let fixture = AuthoringFixture::with_faults(Default::default()).await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    let first_record = fixture.state.get_revision(&first).await.unwrap().unwrap();
    let markers = tempfile::tempdir().unwrap();
    let ready = markers.path().join("ready");
    let release = markers.path().join("release");
    let app_root = fixture
        .store
        .paths()
        .managed
        .parent()
        .unwrap()
        .to_path_buf();
    let cache_root = fixture
        .store
        .paths()
        .staging
        .parent()
        .unwrap()
        .to_path_buf();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("skill_manager_convergence_tests::subprocess_durable_lease_helper")
        .arg("--nocapture")
        .env("AGENTWEAVE_LEASE_APP_ROOT", &app_root)
        .env("AGENTWEAVE_LEASE_CACHE_ROOT", &cache_root)
        .env("AGENTWEAVE_LEASE_READY", &ready)
        .env("AGENTWEAVE_LEASE_RELEASE", &release)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for_path(&ready).await;

    activate_new_revision(&fixture, "2.0.0").await;
    activate_new_revision(&fixture, "3.0.0").await;
    let retained = fixture
        .manager
        .cleanup_unreferenced_revisions()
        .await
        .unwrap();
    assert!(retained.retained_revisions.contains(&first));

    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    let still_retained = fixture
        .manager
        .cleanup_unreferenced_revisions()
        .await
        .unwrap();
    assert!(still_retained.retained_revisions.contains(&first));
    assert!(std::path::Path::new(&first_record.storage_path).is_dir());

    sqlx::query("UPDATE skill_snapshot_leases SET expires_at = '2000-01-01T00:00:00Z'")
        .execute(fixture.state.pool())
        .await
        .unwrap();
    let expired = fixture
        .manager
        .cleanup_unreferenced_revisions()
        .await
        .unwrap();
    assert!(expired.deleted_revisions.contains(&first));
    assert!(!std::path::Path::new(&first_record.storage_path).exists());
}

#[test]
fn subprocess_durable_lease_helper() {
    let Some(app_root) = std::env::var_os("AGENTWEAVE_LEASE_APP_ROOT") else {
        return;
    };
    let cache_root =
        std::path::PathBuf::from(std::env::var_os("AGENTWEAVE_LEASE_CACHE_ROOT").unwrap());
    let ready = std::path::PathBuf::from(std::env::var_os("AGENTWEAVE_LEASE_READY").unwrap());
    let release = std::path::PathBuf::from(std::env::var_os("AGENTWEAVE_LEASE_RELEASE").unwrap());
    let app_root = std::path::PathBuf::from(app_root);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let storage = crate::storage::Storage::connect(&format!(
            "sqlite://{}?mode=rwc",
            app_root.join("state.sqlite").display()
        ))
        .await
        .unwrap();
        let state = crate::skill_state::SkillStateStore::new(storage);
        let paths = crate::skill_store::SkillStorePaths::prepare(&app_root, &cache_root)
            .await
            .unwrap();
        let store = crate::skill_store::SkillRevisionStore::new(paths, state.clone());
        let manager = crate::skill_manager::SkillManager::new_deferred_managed(
            crate::skill_manager::SkillManagerConfig {
                sources: vec![std::sync::Arc::new(
                    crate::skill_source::ManagedSkillSource::from_store(store.clone()),
                )],
                platform: crate::platform::PlatformId::Server,
                capabilities: crate::platform::CapabilitySet::from_names(Vec::<String>::new()),
                protected_packages: Vec::new(),
                allowed_overrides: Vec::new(),
                runtime_version: "0.1.0".parse().unwrap(),
            },
        )
        .await
        .unwrap();
        let _service = crate::skill_management::OwnerSkillManagementService::new(
            manager.clone(),
            store,
            state,
            crate::skill_policy::SkillManagementPolicy::owner_only(),
        );
        manager.startup_reconcile().await.unwrap();
        let _lease = manager.lease_snapshot_for_turn().await.unwrap();
        tokio::fs::write(&ready, b"leased").await.unwrap();
        while !release.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
}

async fn wait_for_path(path: &std::path::Path) {
    for _ in 0..1_000 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("subprocess did not create marker: {}", path.display());
}

fn assert_active_revision(lease: &crate::skill_snapshot::SkillSnapshotLease, expected: &str) {
    let package = lease
        .snapshot()
        .packages()
        .iter()
        .find(|resolved| resolved.package.layer == SkillLayer::Managed)
        .expect("managed package must be active");
    let revision = package
        .package
        .verified_content
        .as_ref()
        .and_then(|content| content.execution_binding.as_ref())
        .map(|binding| binding.revision_id.as_str());
    assert_eq!(revision, Some(expected));
}
