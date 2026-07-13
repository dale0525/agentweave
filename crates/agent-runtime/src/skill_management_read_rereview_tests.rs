use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::{AuthoringFixture, write_package};
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_management::SkillManagementError;
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::SkillPackageKind;
use crate::skill_policy::{SkillGrant, SkillManagementPolicy};
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::{DirectorySkillSource, ManagedSkillSource, SkillLayer};
use crate::skill_state::SkillStateStore;
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use crate::skill_store_public_types::SkillStoreBoundaryError;
use crate::storage::Storage;
use crate::tools::ToolSource;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn revision_drift_on_the_inspection_path_maps_to_conflict() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let observed = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    fixture
        .state
        .update_revision_validation(
            &draft.revision_id,
            json!({"ok": true, "errors": [], "warnings": []}),
        )
        .await
        .unwrap();

    let inspection_error = match fixture.store.inspect_revision_content(&observed).await {
        Ok(_) => panic!("stale inspection unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(
        inspection_error.downcast_ref::<SkillStoreBoundaryError>(),
        Some(SkillStoreBoundaryError::Conflict(_))
    ));
    assert!(matches!(
        SkillManagementError::from_store(
            "inspect skill revision",
            "skill revision",
            inspection_error,
        ),
        SkillManagementError::Conflict { .. }
    ));
}

#[tokio::test]
async fn builtin_only_capability_failure_remains_the_authoritative_effective_layer() {
    let app = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let package_root = builtin.path().join("unavailable");
    write_package(
        &package_root,
        "com.example.unavailable",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = package_root.join("general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["requires"]["capabilities"] = json!(["network.http"]);
    tokio::fs::write(
        &descriptor_path,
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    let state = SkillStateStore::new(Storage::connect("sqlite::memory:").await.unwrap());
    let store = SkillRevisionStore::new(
        SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap(),
        state.clone(),
    );
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![
            Arc::new(DirectorySkillSource::new(
                SkillLayer::Builtin,
                builtin.path(),
            )),
            Arc::new(ManagedSkillSource::from_store(store.clone())),
        ],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap();
    let service = OwnerSkillManagementService::new(
        manager,
        store,
        state,
        SkillManagementPolicy::owner_only(),
    );
    let actor = crate::skill_policy::ActorContext::owner("owner", [SkillGrant::Inspect]);

    let item = service.list_layered_skills(&actor).await.unwrap().remove(0);

    let effective = item
        .effective
        .expect("unavailable resolver outcome must remain effective");
    assert_eq!(effective.source_layer, "builtin");
    assert_eq!(effective.status, "capability_missing");
    assert!(!effective.available);
    assert!(effective.reason.contains("network.http"));
    assert!(item.managed.is_none());
}

#[tokio::test]
async fn circuit_open_runtime_keeps_installation_disable_and_rollback_actions_reachable() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let current = activate_new_revision(&fixture, "2.0.0").await;
    let source = ToolSource::RuntimeSkill {
        skill_name: "calendar".into(),
        package_id: "com.example.calendar".into(),
        revision_id: Some(current.clone()),
    };
    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let actor = fixture.actor([
        SkillGrant::Inspect,
        SkillGrant::Disable,
        SkillGrant::Rollback,
    ]);

    let item = fixture
        .service
        .list_layered_skills(&actor)
        .await
        .unwrap()
        .remove(0);

    let effective = item
        .effective
        .expect("circuit outcome must remain effective");
    assert_eq!(effective.status, "circuit_open");
    assert!(!effective.available);
    assert_eq!(
        effective.active_revision_id.as_deref(),
        Some(current.as_str())
    );
    let installation = item
        .managed
        .expect("active installation facts must remain separate");
    assert_eq!(installation.status, "active");
    assert!(installation.available);
    assert!(item.actions.can_disable);
    assert!(item.actions.can_rollback);
}
