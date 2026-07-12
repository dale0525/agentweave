use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_bundle::{BuildSkillBundleRequest, BundleSkillSource, build_skill_bundle};
use agent_runtime::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService,
};
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::{SkillPackageId, SkillPackageKind};
use agent_runtime::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use agent_runtime::skill_recovery::RecoveryStatus;
use agent_runtime::skill_source::{ManagedSkillSource, SkillLayer, SkillSource};
use agent_runtime::skill_state::{SkillInstallStatus, SkillStateStore};
use agent_runtime::skill_store::{SkillRevisionStore, SkillStorePaths};
use agent_runtime::storage::Storage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[tokio::test]
async fn compatible_managed_skill_survives_builtin_bundle_upgrade() {
    let fixture = UpgradeFixture::new().await;
    fixture.write_builtin_provider("1.0.0", "Provider v1").await;
    fixture.publish_bundle("v1").await;
    let (manager, service) = fixture.manager(Vec::new(), Vec::new()).await;
    manager.startup_reconcile().await.unwrap();
    let _revision = fixture
        .activate_managed(
            &service,
            "com.example.calendar",
            Some("com.example.provider"),
            None,
        )
        .await;

    fixture.write_builtin_provider("2.0.0", "Provider v2").await;
    fixture.publish_bundle("v2").await;
    let (upgraded, _) = fixture.manager(Vec::new(), Vec::new()).await;
    let report = upgraded.startup_reconcile().await.unwrap();

    assert_eq!(report.status, RecoveryStatus::NewSnapshotPublished);
    assert!(active_managed(&upgraded, "com.example.calendar"));
    assert_eq!(
        fixture
            .state
            .get_installation(&package_id("com.example.calendar"))
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillInstallStatus::Active
    );
}

#[tokio::test]
async fn incompatible_managed_skill_is_inactivated_without_deletion_or_startup_failure() {
    let fixture = UpgradeFixture::new().await;
    fixture.write_builtin_provider("1.0.0", "Provider v1").await;
    fixture.publish_bundle("v1").await;
    let (manager, service) = fixture.manager(Vec::new(), Vec::new()).await;
    manager.startup_reconcile().await.unwrap();
    let revision = fixture
        .activate_managed(
            &service,
            "com.example.calendar",
            Some("com.example.provider"),
            None,
        )
        .await;
    let revision_record = fixture
        .state
        .get_revision(&revision)
        .await
        .unwrap()
        .unwrap();

    tokio::fs::remove_dir_all(fixture.source.join("provider"))
        .await
        .unwrap();
    fixture.publish_bundle("v2-removes-provider").await;
    let (upgraded, _) = fixture.manager(Vec::new(), Vec::new()).await;
    let report = upgraded.startup_reconcile().await.unwrap();

    assert_eq!(report.status, RecoveryStatus::NewSnapshotPublished);
    let snapshot = upgraded.current_snapshot();
    let inactive = snapshot
        .inactive()
        .iter()
        .find(|resolved| resolved.package.descriptor.id == package_id("com.example.calendar"))
        .unwrap()
        .reason
        .clone();
    assert!(inactive.contains("missing dependency"), "{inactive}");
    let installation = fixture
        .state
        .get_installation(&package_id("com.example.calendar"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Inactive);
    assert!(!installation.enabled);
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision.as_str())
    );
    assert!(
        fixture
            .state
            .get_revision(&revision)
            .await
            .unwrap()
            .is_some()
    );
    assert!(Path::new(&revision_record.storage_path).is_dir());
    let audit = fixture
        .state
        .list_audit(&package_id("com.example.calendar"))
        .await
        .unwrap();
    let transition = audit
        .iter()
        .find(|entry| entry.operation == "application_update_inactivated")
        .expect("application update status transition must be audited");
    assert_eq!(transition.metadata_json["reason"], inactive);
    upgraded.lease_snapshot_for_turn().await.unwrap();
}

#[tokio::test]
async fn new_protected_package_policy_overrides_a_previous_managed_allowlist() {
    let fixture = UpgradeFixture::new().await;
    fixture
        .write_builtin_shared("1.0.0", "Builtin shared v1")
        .await;
    fixture.publish_bundle("v1").await;
    let shared = package_id("com.example.shared");
    let (manager, service) = fixture.manager(Vec::new(), vec![shared.clone()]).await;
    manager.startup_reconcile().await.unwrap();
    let _revision = fixture
        .activate_managed(&service, shared.as_str(), None, None)
        .await;
    assert!(active_managed(&manager, shared.as_str()));

    fixture
        .write_builtin_shared("2.0.0", "Builtin shared v2")
        .await;
    fixture.publish_bundle("v2-protected").await;
    let (upgraded, _) = fixture
        .manager(vec![shared.clone()], vec![shared.clone()])
        .await;
    upgraded.startup_reconcile().await.unwrap();

    assert!(!active_managed(&upgraded, shared.as_str()));
    let snapshot = upgraded.current_snapshot();
    let inactive = snapshot
        .inactive()
        .iter()
        .find(|resolved| {
            resolved.package.descriptor.id == shared
                && resolved.package.layer == SkillLayer::Managed
        })
        .unwrap();
    assert!(inactive.reason.contains("protected"));
    assert_eq!(
        fixture
            .state
            .get_installation(&shared)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillInstallStatus::Inactive
    );
}

#[tokio::test]
async fn untrusted_network_skill_is_inactivated_even_when_the_application_graph_is_unchanged() {
    let fixture = UpgradeFixture::new().await;
    fixture.publish_bundle("v1-empty").await;
    let (manager, service) = fixture.manager(Vec::new(), Vec::new()).await;
    manager.startup_reconcile().await.unwrap();
    let revision = fixture
        .activate_managed(&service, "com.example.network", None, Some("network.http"))
        .await;
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=rw",
        fixture.database_path.display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE skill_installations SET trust_level = 'untrusted' WHERE package_id = ?")
        .bind("com.example.network")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let (restarted, _) = fixture.manager(Vec::new(), Vec::new()).await;
    let report = restarted.startup_reconcile().await.unwrap();

    assert_eq!(report.status, RecoveryStatus::NewSnapshotPublished);
    let snapshot = restarted.current_snapshot();
    let inactive = snapshot
        .inactive()
        .iter()
        .find(|resolved| {
            resolved.package.descriptor.id == package_id("com.example.network")
                && resolved.package.layer == SkillLayer::Managed
        })
        .unwrap();
    assert!(inactive.reason.contains("network policy"));
    let installation = fixture
        .state
        .get_installation(&package_id("com.example.network"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Inactive);
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision.as_str())
    );
}

#[tokio::test]
async fn failed_upgrade_transaction_rolls_back_every_status_and_retries_cleanly() {
    let fixture = UpgradeFixture::new().await;
    fixture.write_builtin_provider("1.0.0", "Provider v1").await;
    fixture.publish_bundle("v1").await;
    let (manager, service) = fixture.manager(Vec::new(), Vec::new()).await;
    manager.startup_reconcile().await.unwrap();
    fixture
        .activate_managed(
            &service,
            "com.example.calendar",
            Some("com.example.provider"),
            None,
        )
        .await;
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}?mode=rw",
        fixture.database_path.display()
    ))
    .await
    .unwrap();
    let before_generation: i64 =
        sqlx::query_scalar("SELECT generation FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_application_update_audit
           BEFORE INSERT ON skill_audit_log
           WHEN NEW.operation = 'application_update_inactivated'
           BEGIN SELECT RAISE(ABORT, 'audit blocked'); END"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    tokio::fs::remove_dir_all(fixture.source.join("provider"))
        .await
        .unwrap();
    fixture.publish_bundle("v2").await;
    let (restarted, _) = fixture.manager(Vec::new(), Vec::new()).await;

    assert!(restarted.startup_reconcile().await.is_err());

    let installation = fixture
        .state
        .get_installation(&package_id("com.example.calendar"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Active);
    assert!(installation.enabled);
    let after_failure_generation: i64 =
        sqlx::query_scalar("SELECT generation FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after_failure_generation, before_generation);

    sqlx::query("DROP TRIGGER fail_application_update_audit")
        .execute(&pool)
        .await
        .unwrap();
    let retry = restarted.startup_reconcile().await.unwrap();

    assert_eq!(retry.status, RecoveryStatus::NewSnapshotPublished);
    assert_eq!(
        fixture
            .state
            .get_installation(&package_id("com.example.calendar"))
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillInstallStatus::Inactive
    );
    pool.close().await;
}

struct UpgradeFixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    bundle: PathBuf,
    app: PathBuf,
    cache: PathBuf,
    storage: Storage,
    state: SkillStateStore,
    database_path: PathBuf,
}

impl UpgradeFixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let bundle = root.path().join("bundle");
        let app = root.path().join("app");
        let cache = root.path().join("cache");
        tokio::fs::create_dir_all(&source).await.unwrap();
        let database_path = root.path().join("state.db");
        let storage = Storage::connect(&format!("sqlite://{}?mode=rwc", database_path.display()))
            .await
            .unwrap();
        let state = SkillStateStore::new(storage.clone());
        Self {
            _root: root,
            source,
            bundle,
            app,
            cache,
            storage,
            state,
            database_path,
        }
    }

    async fn manager(
        &self,
        protected_packages: Vec<SkillPackageId>,
        allowed_overrides: Vec<SkillPackageId>,
    ) -> (SkillManager, OwnerSkillManagementService) {
        let paths = SkillStorePaths::prepare(&self.app, &self.cache)
            .await
            .unwrap();
        let store = SkillRevisionStore::new(paths, SkillStateStore::new(self.storage.clone()));
        let sources: Vec<Arc<dyn SkillSource>> = vec![
            Arc::new(BundleSkillSource::open(&self.bundle).await.unwrap()),
            Arc::new(ManagedSkillSource::from_store(store.clone())),
        ];
        let manager = SkillManager::new_deferred_managed(SkillManagerConfig {
            sources,
            platform: PlatformId::Desktop,
            capabilities: CapabilitySet::desktop_runtime(),
            protected_packages: protected_packages.clone(),
            allowed_overrides: allowed_overrides.clone(),
            runtime_version: "0.1.0".parse().unwrap(),
        })
        .await
        .unwrap();
        let mut policy = SkillManagementPolicy::owner_only();
        policy.protected_packages = protected_packages.into_iter().collect();
        policy.allowed_overrides = allowed_overrides.into_iter().collect();
        let service = OwnerSkillManagementService::new(
            manager.clone(),
            store,
            SkillStateStore::new(self.storage.clone()),
            policy,
        );
        (manager, service)
    }

    async fn publish_bundle(&self, generated_at: &str) {
        build_skill_bundle(BuildSkillBundleRequest {
            source_roots: vec![self.source.clone()],
            output_root: self.bundle.clone(),
            platform: PlatformId::Desktop,
            runtime_version: "0.1.0".parse().unwrap(),
            generated_at: generated_at.into(),
        })
        .await
        .unwrap();
    }

    async fn write_builtin_provider(&self, version: &str, body: &str) {
        write_instruction_package(
            &self.source.join("provider"),
            "com.example.provider",
            version,
            body,
        )
        .await;
    }

    async fn write_builtin_shared(&self, version: &str, body: &str) {
        write_instruction_package(
            &self.source.join("shared"),
            "com.example.shared",
            version,
            body,
        )
        .await;
    }

    async fn activate_managed(
        &self,
        service: &OwnerSkillManagementService,
        id: &str,
        dependency: Option<&str>,
        capability: Option<&str>,
    ) -> String {
        let actor = owner("requester");
        let descriptor = serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": "Managed fixture",
            "kind": "instruction_only",
            "package": {"includeInstructions": true, "includeRuntime": false},
            "compatibility": {"platforms": ["desktop"]},
            "requires": {
                "packages": dependency.into_iter().collect::<Vec<_>>(),
                "capabilities": capability.into_iter().collect::<Vec<_>>(),
                "runtimeTools": [], "connectors": []
            }
        });
        let draft = service
            .create_draft_with_files(
                &actor,
                CreateSkillDraftRequest {
                    package_id: package_id(id),
                    display_name: "Managed fixture".into(),
                    description: "Managed fixture body.".into(),
                    kind: SkillPackageKind::InstructionOnly,
                    required_tools: Vec::new(),
                },
                vec![
                    DraftFileUpdate {
                        path: "general-agent.json".into(),
                        content: format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
                    },
                    DraftFileUpdate {
                        path: "SKILL.md".into(),
                        content: "---\nname: managed-fixture\ndescription: Managed fixture.\n---\n\nManaged fixture body.\n".into(),
                    },
                ],
            )
            .await
            .unwrap();
        service
            .validate_draft(&actor, &draft.revision_id)
            .await
            .unwrap();
        let approval = service
            .request_activation(&actor, &draft.revision_id)
            .await
            .unwrap();
        service
            .approve_activation(&approval.approval_id, &owner("approver"))
            .await
            .unwrap();
        draft.revision_id
    }
}

fn owner(id: &str) -> ActorContext {
    ActorContext::owner(
        id,
        [
            SkillGrant::Inspect,
            SkillGrant::CreateDraft,
            SkillGrant::EditDraft,
            SkillGrant::Validate,
            SkillGrant::Activate,
            SkillGrant::OverrideBuiltin,
        ],
    )
}

fn package_id(value: &str) -> SkillPackageId {
    SkillPackageId::parse(value).unwrap()
}

fn active_managed(manager: &SkillManager, package: &str) -> bool {
    manager
        .current_snapshot()
        .packages()
        .iter()
        .any(|resolved| {
            resolved.package.layer == SkillLayer::Managed
                && resolved.package.descriptor.id.as_str() == package
        })
}

async fn write_instruction_package(root: &Path, id: &str, version: &str, body: &str) {
    tokio::fs::create_dir_all(root).await.unwrap();
    let descriptor = serde_json::json!({
        "schemaVersion": 1,
        "id": id,
        "version": version,
        "displayName": id,
        "kind": "instruction_only",
        "package": {"includeInstructions": true, "includeRuntime": false},
        "compatibility": {"platforms": ["desktop"]},
        "requires": {"packages": [], "capabilities": [], "runtimeTools": [], "connectors": []}
    });
    tokio::fs::write(
        root.join("general-agent.json"),
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.join("SKILL.md"),
        format!(
            "---\nname: {}\ndescription: Builtin fixture.\n---\n\n{body}\n",
            id.replace('.', "-")
        ),
    )
    .await
    .unwrap();
}
