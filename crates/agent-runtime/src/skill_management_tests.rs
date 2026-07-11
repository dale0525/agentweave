use crate::skill::SkillRegistry;
use crate::skill_authoring::build_package_draft;
use crate::skill_catalog::SkillCatalog;
use crate::skill_management::{CreateSkillDraftRequest, OwnerSkillManagementService};
use crate::skill_management_tools::{SkillManagementToolContext, SkillManagementTools};
use crate::skill_manager::SkillManager;
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementMode, SkillManagementPolicy};
use crate::skill_state::{SkillLayerRecord, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use crate::tools::{RuntimeConfig, ToolRegistry};
use serde_json::json;
use std::collections::BTreeSet;
use tempfile::{TempDir, tempdir};

struct ManagementFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    store: SkillRevisionStore,
    service: OwnerSkillManagementService,
}

impl ManagementFixture {
    async fn new(policy: SkillManagementPolicy) -> Self {
        Self::with_limits_and_faults(policy, SkillStoreLimits::default(), Default::default()).await
    }

    async fn with_limits_and_faults(
        policy: SkillManagementPolicy,
        limits: SkillStoreLimits,
        faults: SkillStoreTestFaults,
    ) -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage.clone());
        let store = SkillRevisionStore::with_test_faults(paths, state.clone(), limits, faults);
        let manager =
            SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty());
        let service =
            OwnerSkillManagementService::new(manager, store.clone(), state.clone(), policy);
        Self {
            _app: app,
            _cache: cache,
            storage,
            state,
            store,
            service,
        }
    }

    fn owner(&self) -> ActorContext {
        ActorContext::owner("owner-1", [SkillGrant::Inspect, SkillGrant::CreateDraft])
    }
}

fn request(kind: SkillPackageKind) -> CreateSkillDraftRequest {
    CreateSkillDraftRequest {
        package_id: SkillPackageId::parse("com.example.calendar").unwrap(),
        display_name: "Calendar".into(),
        description: "Calendar workflow.".into(),
        kind,
        required_tools: if kind == SkillPackageKind::HostToolsOnly {
            vec!["write_file".into(), "read_file".into(), "write_file".into()]
        } else {
            Vec::new()
        },
    }
}

#[tokio::test]
async fn ordinary_actor_cannot_create_a_draft() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;

    let error = fixture
        .service
        .create_draft(
            &ActorContext::anonymous(),
            request(SkillPackageKind::InstructionOnly),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("skills.create_draft denied"));
    assert!(directory_is_empty(&fixture.store.paths().staging).await);
}

#[tokio::test]
async fn diagnostics_actor_can_inspect_without_a_package_kind() {
    let policy = SkillManagementPolicy {
        mode: SkillManagementMode::DiagnosticsOnly,
        ..SkillManagementPolicy::default()
    };
    let fixture = ManagementFixture::new(policy).await;
    let actor = ActorContext::anonymous().with_grants([SkillGrant::Inspect]);

    assert!(fixture.service.list_effective_skills(&actor).await.is_ok());
    assert!(fixture.service.list_managed_skills(&actor).await.is_ok());
    assert!(
        fixture
            .service
            .list_effective_skills(&ActorContext::anonymous())
            .await
            .unwrap_err()
            .to_string()
            .contains("skills.inspect denied")
    );
}

#[test]
fn draft_bytes_are_deterministic_sorted_and_front_matter_safe() {
    let mut draft = request(SkillPackageKind::HostToolsOnly);
    draft.display_name = "Calendar: \"safe\"".into();
    draft.description = "line one\n---\nrole: owner".into();

    let first = build_package_draft(&draft).unwrap();
    let second = build_package_draft(&draft).unwrap();

    assert_eq!(first, second);
    let descriptor: serde_json::Value = serde_json::from_slice(first.descriptor_bytes()).unwrap();
    assert_eq!(
        descriptor["requires"]["runtimeTools"],
        json!(["read_file", "write_file"])
    );
    let skill = std::str::from_utf8(first.instructions_bytes()).unwrap();
    assert!(skill.contains("description: \"line one\\n---\\nrole: owner\""));
    assert_eq!(skill.matches("\n---\n").count(), 1);
}

#[tokio::test]
async fn authorized_draft_uses_one_uuid_for_tree_and_record() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;

    let summary = fixture
        .service
        .create_draft(&fixture.owner(), request(SkillPackageKind::InstructionOnly))
        .await
        .unwrap();
    let record = fixture
        .state
        .get_revision(&summary.revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record.revision_id, summary.revision_id);
    assert_eq!(
        std::path::Path::new(&record.storage_path)
            .file_name()
            .unwrap(),
        std::ffi::OsStr::new(&summary.revision_id)
    );
    assert_eq!(record.package_id, summary.package_id);
    assert_eq!(record.descriptor_json["kind"], "instruction_only");
    assert_eq!(record.validation_json, json!({"status": "pending"}));
}

#[tokio::test]
async fn authoring_write_failure_removes_unrecorded_reserved_tree() {
    let faults = SkillStoreTestFaults::default();
    faults.fail_once(SkillStoreFaultPoint::StagingAuthorFile);
    let fixture = ManagementFixture::with_limits_and_faults(
        SkillManagementPolicy::owner_only(),
        SkillStoreLimits::default(),
        faults,
    )
    .await;

    let error = fixture
        .service
        .create_draft(&fixture.owner(), request(SkillPackageKind::InstructionOnly))
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("StagingAuthorFile"));
    assert!(directory_is_empty(&fixture.store.paths().staging).await);
    assert_eq!(revision_count(&fixture.storage).await, 0);
}

#[tokio::test]
async fn staging_record_failure_removes_unrecorded_reserved_tree() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;
    sqlx::query("DROP TABLE skill_revisions")
        .execute(fixture.storage.pool())
        .await
        .unwrap();

    fixture
        .service
        .create_draft(&fixture.owner(), request(SkillPackageKind::InstructionOnly))
        .await
        .unwrap_err();

    assert!(directory_is_empty(&fixture.store.paths().staging).await);
}

#[tokio::test]
async fn validation_happens_before_staging_reservation() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;
    let mut invalid = request(SkillPackageKind::HostToolsOnly);
    invalid.required_tools = vec!["../../command".into()];

    let error = fixture
        .service
        .create_draft(&fixture.owner(), invalid)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("invalid required tool"));
    assert!(directory_is_empty(&fixture.store.paths().staging).await);
}

#[test]
fn disabled_policy_exposes_no_management_tool_definitions() {
    let manager =
        SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty());
    let policy = SkillManagementPolicy::default();
    let actor = ActorContext::owner("owner-1", [SkillGrant::CreateDraft]);

    assert!(SkillManagementTools::definitions_for_policy(&policy, &actor).is_empty());
    drop(manager);
}

#[tokio::test]
async fn only_executable_initial_management_tool_is_advertised_and_dispatched() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;
    let context = SkillManagementToolContext {
        service: fixture.service.clone(),
        actor: fixture.owner(),
    };
    let config = RuntimeConfig::read_only(".", ".").without_builtin_tools();
    let registry =
        ToolRegistry::try_new_with_management(SkillRegistry::empty(), &config, Some(context))
            .unwrap();

    let names = registry
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect::<BTreeSet<_>>();
    assert_eq!(names, BTreeSet::from(["create_skill_draft".to_string()]));

    let result = registry
        .execute(
            "create_skill_draft",
            "call-1",
            json!({
                "package_id": "com.example.tool-created",
                "display_name": "Tool created",
                "description": "Created by the management dispatcher.",
                "kind": "instruction_only",
                "required_tools": [],
                "actor": {"actor_id": "attacker", "role": "owner", "grants": ["create_draft"]}
            }),
        )
        .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_arguments");
    assert_eq!(revision_count(&fixture.storage).await, 0);
}

#[tokio::test]
async fn management_dispatch_uses_host_actor_not_argument_identity() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;
    let context = SkillManagementToolContext {
        service: fixture.service.clone(),
        actor: ActorContext::anonymous(),
    };
    let result = SkillManagementTools::execute(
        &context,
        "create_skill_draft",
        "call-2",
        json!({
            "package_id": "com.example.spoofed",
            "display_name": "Spoofed",
            "description": "Must be denied.",
            "kind": "instruction_only",
            "required_tools": []
        }),
    )
    .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "permission_denied");
    assert_eq!(revision_count(&fixture.storage).await, 0);
}

#[tokio::test]
async fn authorized_actor_can_list_managed_skills_and_audit() {
    let fixture = ManagementFixture::new(SkillManagementPolicy::owner_only()).await;
    let summary = fixture
        .service
        .create_draft(&fixture.owner(), request(SkillPackageKind::InstructionOnly))
        .await
        .unwrap();
    let promoted = fixture
        .store
        .promote_revision(&summary.revision_id)
        .await
        .unwrap();
    fixture
        .state
        .activate_revision(
            &summary.package_id,
            &promoted.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let managed = fixture
        .service
        .list_managed_skills(&fixture.owner())
        .await
        .unwrap();
    let audit = fixture
        .service
        .list_audit(&fixture.owner(), &summary.package_id)
        .await
        .unwrap();

    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0].package_id, summary.package_id);
    assert_eq!(managed[0].active_revision_id, Some(promoted.revision_id));
    assert!(
        audit
            .iter()
            .any(|record| record.operation == "activate_revision")
    );
}

async fn directory_is_empty(path: &std::path::Path) -> bool {
    tokio::fs::read_dir(path)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none()
}

async fn revision_count(storage: &Storage) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(storage.pool())
        .await
        .unwrap()
}
