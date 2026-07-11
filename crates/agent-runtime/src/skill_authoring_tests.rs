use crate::events::RuntimeEvent;
use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService, SkillDraftValidation,
};
use crate::skill_management_tools::{SkillManagementToolContext, SkillManagementTools};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use crate::skill_source::ManagedSkillSource;
use crate::skill_state::{
    SkillApprovalStatus, SkillLayerRecord, SkillRevisionStatus, SkillStateStore,
};
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use crate::storage::Storage;
use crate::tools::{RuntimeConfig, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};

struct AuthoringFixture {
    _app: TempDir,
    _cache: TempDir,
    imports: TempDir,
    exports: TempDir,
    state: SkillStateStore,
    store: SkillRevisionStore,
    manager: SkillManager,
    service: OwnerSkillManagementService,
}

impl AuthoringFixture {
    async fn new() -> Self {
        Self::with_policy(SkillManagementPolicy::owner_only()).await
    }

    async fn with_policy(policy: SkillManagementPolicy) -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let imports = tempdir().unwrap();
        let exports = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage);
        let store = SkillRevisionStore::new(paths, state.clone());
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
        let service =
            OwnerSkillManagementService::new(manager.clone(), store.clone(), state.clone(), policy)
                .with_transfer_roots(imports.path(), exports.path())
                .unwrap();
        Self {
            _app: app,
            _cache: cache,
            imports,
            exports,
            state,
            store,
            manager,
            service,
        }
    }

    fn actor(&self, grants: impl IntoIterator<Item = SkillGrant>) -> ActorContext {
        ActorContext::owner("owner-1", grants)
    }

    async fn draft(&self) -> crate::skill_management::SkillDraftSummary {
        self.service
            .create_draft(
                &self.actor([SkillGrant::CreateDraft]),
                CreateSkillDraftRequest {
                    package_id: SkillPackageId::parse("com.example.calendar").unwrap(),
                    display_name: "Calendar".into(),
                    description: "Guide calendar planning.".into(),
                    kind: SkillPackageKind::InstructionOnly,
                    required_tools: Vec::new(),
                },
            )
            .await
            .unwrap()
    }
}

async fn write_package(root: &std::path::Path, id: &str, kind: SkillPackageKind) {
    tokio::fs::create_dir_all(root).await.unwrap();
    let authored = crate::skill_authoring::build_package_draft(&CreateSkillDraftRequest {
        package_id: SkillPackageId::parse(id).unwrap(),
        display_name: "Imported".into(),
        description: "Imported package.".into(),
        kind: SkillPackageKind::InstructionOnly,
        required_tools: Vec::new(),
    })
    .unwrap();
    for file in authored.files() {
        tokio::fs::write(root.join(&file.path), &file.bytes)
            .await
            .unwrap();
    }
    if kind == SkillPackageKind::NativeRuntime {
        let descriptor_path = root.join("general-agent.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
        value["kind"] = json!("native_runtime");
        value["package"]["includeRuntime"] = json!(true);
        tokio::fs::write(
            descriptor_path,
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
        )
        .await
        .unwrap();
        tokio::fs::write(
            root.join("skill.json"),
            br#"{"name":"native","description":"no","version":"0.1.0","entry":{"type":"process","command":"false"},"tools":[{"name":"native_tool","description":"no","input_schema":{"type":"object"}}]}"#,
        )
        .await
        .unwrap();
    }
}

fn update(path: &str, content: impl Into<String>) -> DraftFileUpdate {
    DraftFileUpdate {
        path: PathBuf::from(path),
        content: content.into(),
    }
}

#[tokio::test]
async fn draft_update_rejects_disallowed_paths_and_oversized_files_without_mutation() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let before = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let actor = fixture.actor([SkillGrant::EditDraft]);

    for files in [
        vec![update("../escape", "bad")],
        vec![update("nested/SKILL.md", "bad")],
        vec![update("assets/too-large.txt", "x".repeat(256 * 1024 + 1))],
    ] {
        let error = fixture
            .service
            .update_draft(&actor, &draft.revision_id, files)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("draft") || error.to_string().contains("256 KiB"),
            "{error:#}"
        );
    }

    let after = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.content_hash, before.content_hash);
    assert_eq!(after.storage_path, before.storage_path);
    assert_eq!(after.validation_json, before.validation_json);
}

#[tokio::test]
async fn draft_update_commits_all_files_and_refreshes_metadata_once() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let before = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    fixture
        .state
        .update_revision_validation(
            &draft.revision_id,
            json!({"status": "valid", "ok": true, "test": {"ok": true}}),
        )
        .await
        .unwrap();

    let updated = fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![
                update(
                    "SKILL.md",
                    "---\nname: com-example-calendar\ndescription: Updated\n---\n\n# Updated\n",
                ),
                update("references/guide.md", "Use the calendar carefully.\n"),
            ],
        )
        .await
        .unwrap();
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(updated.revision_id, draft.revision_id);
    assert_ne!(record.content_hash, before.content_hash);
    assert_ne!(record.storage_path, before.storage_path);
    assert_eq!(record.validation_json, json!({"status": "pending"}));
    assert_eq!(
        tokio::fs::read_to_string(PathBuf::from(&record.storage_path).join("references/guide.md"))
            .await
            .unwrap(),
        "Use the calendar carefully.\n"
    );
    assert!(
        tokio::fs::read_to_string(PathBuf::from(&record.storage_path).join("SKILL.md"))
            .await
            .unwrap()
            .contains("# Updated")
    );
}

#[tokio::test]
async fn draft_update_authorizes_before_reading_revision_state() {
    let fixture = AuthoringFixture::new().await;
    let error = fixture
        .service
        .update_draft(
            &fixture.actor([]),
            "not-a-revision-id",
            vec![update("SKILL.md", "ignored")],
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("skills.edit_draft denied"));
}

#[tokio::test]
async fn host_tools_draft_rejects_unknown_required_tools_before_staging() {
    let fixture = AuthoringFixture::new().await;
    let error = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.host-tools").unwrap(),
                display_name: "Host tools".into(),
                description: "Use a host tool.".into(),
                kind: SkillPackageKind::HostToolsOnly,
                required_tools: vec!["com.example.missing/create_event".into()],
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unknown required host tool"));
}

#[tokio::test]
async fn validation_is_deterministic_persisted_and_bound_to_one_snapshot() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let actor = fixture.actor([SkillGrant::Validate]);

    let first = fixture
        .service
        .validate_draft(&actor, &draft.revision_id)
        .await
        .unwrap();
    let second = fixture
        .service
        .validate_draft(&actor, &draft.revision_id)
        .await
        .unwrap();
    let persisted = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();

    assert!(first.ok, "{:?}", first.errors);
    assert_eq!(first, second);
    assert_eq!(persisted, serde_json::to_value(&first).unwrap());
    assert_eq!(first.snapshot_generation, 1);
    assert!(!first.content_hash.is_empty());
    assert_eq!(
        first.permission_diff,
        json!({"addedCapabilities": [], "addedTools": []})
    );
}

#[tokio::test]
async fn validation_reports_catalog_dependency_capability_and_protected_policy_errors() {
    let package_id = SkillPackageId::parse("com.example.calendar").unwrap();
    let fixture = AuthoringFixture::with_policy(
        SkillManagementPolicy::owner_only().protect(package_id.clone()),
    )
    .await;
    let draft = fixture.draft().await;
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let mut descriptor = record.descriptor_json;
    descriptor["requires"]["packages"] = json!(["com.example.missing"]);
    descriptor["requires"]["capabilities"] = json!(["calendar.write"]);
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![
                update(
                    "general-agent.json",
                    format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
                ),
                update("SKILL.md", "not valid front matter\n"),
            ],
        )
        .await
        .unwrap();

    let validation: SkillDraftValidation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();

    assert!(!validation.ok);
    assert_eq!(validation.dependencies, vec!["com.example.missing"]);
    assert_eq!(validation.required_capabilities, vec!["calendar.write"]);
    assert_eq!(
        validation.permission_diff,
        json!({
            "addedCapabilities": ["calendar.write"],
            "addedTools": []
        })
    );
    let joined = validation.errors.join("\n");
    assert!(joined.contains("catalog"), "{joined}");
    assert!(joined.contains("missing dependency"), "{joined}");
    assert!(joined.contains("missing capability"), "{joined}");
    assert!(joined.contains("protected package"), "{joined}");
    assert!(validation.errors.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[tokio::test]
async fn testing_a_validated_draft_records_a_bounded_result_without_publication() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let generation = fixture.manager.current_snapshot().generation();

    let result = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), &draft.revision_id)
        .await
        .unwrap();

    assert!(result.ok);
    assert_eq!(result.error_class, None);
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    let persisted = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();
    assert_eq!(persisted["test"]["ok"], true);
    assert_eq!(persisted["test"]["errorClass"], serde_json::Value::Null);
}

#[tokio::test]
async fn third_party_import_is_bounded_and_stays_quarantined() {
    let fixture = AuthoringFixture::new().await;
    write_package(
        &fixture.imports.path().join("calendar"),
        "com.example.imported",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let generation = fixture.manager.current_snapshot().generation();

    let imported = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            std::path::Path::new("calendar"),
        )
        .await
        .unwrap();
    let record = fixture
        .state
        .get_revision(&imported.revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(imported.status, "quarantined");
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    assert!(
        !fixture
            .manager
            .current_snapshot()
            .packages()
            .iter()
            .any(|item| item.package.descriptor.id.as_str() == "com.example.imported")
    );
}

#[tokio::test]
async fn import_rejects_native_payloads_links_and_hard_links_without_rows() {
    let fixture = AuthoringFixture::new().await;
    write_package(
        &fixture.imports.path().join("native"),
        "com.example.native",
        SkillPackageKind::NativeRuntime,
    )
    .await;
    let actor = fixture.actor([SkillGrant::Import]);
    let native = fixture
        .service
        .import_draft(&actor, std::path::Path::new("native"))
        .await
        .unwrap_err();
    assert!(native.to_string().contains("native runtime"), "{native:#}");

    #[cfg(unix)]
    {
        write_package(
            &fixture.imports.path().join("linked"),
            "com.example.linked",
            SkillPackageKind::InstructionOnly,
        )
        .await;
        std::os::unix::fs::symlink(
            fixture.imports.path().join("linked/SKILL.md"),
            fixture.imports.path().join("linked/assets-link"),
        )
        .unwrap();
        let linked = fixture
            .service
            .import_draft(&actor, std::path::Path::new("linked"))
            .await
            .unwrap_err();
        assert!(linked.to_string().contains("symlink"), "{linked:#}");

        write_package(
            &fixture.imports.path().join("hard-linked"),
            "com.example.hard-linked",
            SkillPackageKind::InstructionOnly,
        )
        .await;
        std::fs::hard_link(
            fixture.imports.path().join("hard-linked/SKILL.md"),
            fixture.imports.path().join("hard-linked/alias.md"),
        )
        .unwrap();
        let hard_linked = fixture
            .service
            .import_draft(&actor, std::path::Path::new("hard-linked"))
            .await
            .unwrap_err();
        assert!(
            hard_linked.to_string().contains("hard link"),
            "{hard_linked:#}"
        );
    }
}

#[tokio::test]
async fn imported_revision_leaves_quarantine_only_after_successful_validation() {
    let fixture = AuthoringFixture::new().await;
    write_package(
        &fixture.imports.path().join("valid-import"),
        "com.example.valid-import",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let valid = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            std::path::Path::new("valid-import"),
        )
        .await
        .unwrap();
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &valid.revision_id)
        .await
        .unwrap();
    assert!(validation.ok);
    assert_eq!(
        fixture
            .state
            .get_revision(&valid.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );

    write_package(
        &fixture.imports.path().join("invalid-import"),
        "com.example.invalid-import",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    tokio::fs::write(
        fixture.imports.path().join("invalid-import/SKILL.md"),
        "invalid front matter",
    )
    .await
    .unwrap();
    let invalid = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            std::path::Path::new("invalid-import"),
        )
        .await
        .unwrap();
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &invalid.revision_id)
        .await
        .unwrap();
    assert!(!validation.ok);
    assert_eq!(
        fixture
            .state
            .get_revision(&invalid.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Quarantined
    );
}

#[tokio::test]
async fn export_copies_exact_active_revision_without_mutating_state() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let promoted = fixture
        .store
        .promote_revision(&draft.revision_id)
        .await
        .unwrap();
    fixture
        .state
        .activate_revision(
            &draft.package_id,
            &promoted.revision_id,
            SkillLayerRecord::Managed,
            "approver-2",
        )
        .await
        .unwrap();
    let before_revision = fixture
        .state
        .get_revision(&promoted.revision_id)
        .await
        .unwrap()
        .unwrap();
    let before_installation = fixture
        .state
        .get_installation(&draft.package_id)
        .await
        .unwrap();

    let exported = fixture
        .service
        .export_managed_skill(
            &fixture.actor([SkillGrant::Export]),
            &draft.package_id,
            std::path::Path::new("calendar"),
        )
        .await
        .unwrap();

    assert_eq!(exported, fixture.exports.path().join("calendar"));
    assert!(exported.join("general-agent.json").is_file());
    assert_eq!(
        fixture
            .state
            .get_revision(&promoted.revision_id)
            .await
            .unwrap()
            .unwrap(),
        before_revision
    );
    assert_eq!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap(),
        before_installation
    );

    for destination in [
        std::path::Path::new("../escape"),
        std::path::Path::new("calendar"),
    ] {
        assert!(
            fixture
                .service
                .export_managed_skill(
                    &fixture.actor([SkillGrant::Export]),
                    &draft.package_id,
                    destination,
                )
                .await
                .is_err()
        );
    }
}

async fn validate_for_activation(
    fixture: &AuthoringFixture,
) -> crate::skill_management::SkillDraftSummary {
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    draft
}

#[tokio::test]
async fn activation_request_requires_validation_and_deduplicates_exact_candidate() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let requester = fixture.actor([SkillGrant::Activate]);
    assert!(
        fixture
            .service
            .request_activation(&requester, &draft.revision_id)
            .await
            .unwrap_err()
            .to_string()
            .contains("validation")
    );
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        fixture
            .service
            .request_activation(&requester, &draft.revision_id),
        fixture
            .service
            .request_activation(&requester, &draft.revision_id),
    );
    let first = first.unwrap();
    let second = second.unwrap();

    assert_eq!(first.approval_id, second.approval_id);
    assert_eq!(first.status, SkillApprovalStatus::Pending);
    assert_eq!(first.requested_by, "owner-1");
    assert_eq!(
        first.permission_diff["binding"]["contentHash"],
        json!(
            fixture
                .state
                .get_revision(&draft.revision_id)
                .await
                .unwrap()
                .unwrap()
                .content_hash
        )
    );
    assert_eq!(
        fixture
            .service
            .emitted_events()
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::SkillApprovalRequired { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn approval_requires_different_actor_is_single_use_and_publishes_once() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let requester = fixture.actor([SkillGrant::Activate]);
    let approval = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    let self_error = fixture
        .service
        .approve_activation(&approval.approval_id, &requester)
        .await
        .unwrap_err();
    assert!(self_error.to_string().contains("own request"));
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);

    let report = fixture
        .service
        .approve_activation(&approval.approval_id, &approver)
        .await
        .unwrap();

    assert_eq!(report.previous_generation, 1);
    assert_eq!(report.active_generation, 2);
    assert!(
        fixture
            .manager
            .current_snapshot()
            .packages()
            .iter()
            .any(|item| { item.package.descriptor.id == draft.package_id })
    );
    assert!(
        fixture
            .service
            .approve_activation(&approval.approval_id, &approver)
            .await
            .unwrap_err()
            .to_string()
            .contains("already resolved")
    );
    assert_eq!(
        fixture
            .service
            .emitted_events()
            .iter()
            .filter(|event| matches!(
                event,
                RuntimeEvent::SkillSnapshotPublished { generation: 2 }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn edit_makes_old_approval_stale_and_new_request_gets_new_binding() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let requester = fixture.actor([SkillGrant::Activate]);
    let old = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![update("references/change.md", "changed\n")],
        )
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);
    assert!(
        fixture
            .service
            .approve_activation(&old.approval_id, &approver)
            .await
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let new = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    assert_ne!(new.approval_id, old.approval_id);
}

#[tokio::test]
async fn concurrent_approval_publishes_one_generation() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);
    let (left, right) = tokio::join!(
        fixture
            .service
            .approve_activation(&approval.approval_id, &approver),
        fixture
            .service
            .approve_activation(&approval.approval_id, &approver),
    );

    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(fixture.manager.current_snapshot().generation(), 2);
}

#[tokio::test]
async fn reload_failure_keeps_old_snapshot_and_installation() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    sqlx::query("DROP TABLE skill_snapshots")
        .execute(fixture.state.pool())
        .await
        .unwrap();
    let error = fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("skill_snapshots"), "{error:#}");
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .state
            .get_approval(&approval.approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Rejected
    );
    assert!(
        !fixture
            .service
            .emitted_events()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::SkillSnapshotPublished { .. }))
    );
}

#[tokio::test]
async fn model_surface_exposes_only_implemented_authoring_tools_by_grant() {
    let fixture = AuthoringFixture::new().await;
    let actor = fixture.actor([
        SkillGrant::CreateDraft,
        SkillGrant::EditDraft,
        SkillGrant::Validate,
        SkillGrant::Test,
        SkillGrant::Activate,
        SkillGrant::Import,
        SkillGrant::Export,
        SkillGrant::Disable,
        SkillGrant::Rollback,
    ]);
    let names = SkillManagementTools::definitions(&fixture.service, &actor)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "create_skill_draft".to_string(),
            "request_skill_activation".to_string(),
            "test_skill_draft".to_string(),
            "update_skill_draft".to_string(),
            "validate_skill_draft".to_string(),
        ])
    );
    assert!(!names.iter().any(|name| {
        name.contains("import")
            || name.contains("export")
            || name.contains("approve")
            || name.contains("disable")
            || name.contains("rollback")
    }));
}

#[tokio::test]
async fn side_effecting_management_tool_uses_host_actor_and_bypasses_post_commit_output_limit() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let actor = fixture.actor([SkillGrant::EditDraft]);
    let context = SkillManagementToolContext {
        service: fixture.service.clone(),
        actor,
    };
    let spoofed = SkillManagementTools::execute(
        &context,
        "update_skill_draft",
        "spoof",
        json!({
            "revision_id": draft.revision_id,
            "files": [{"path": "references/spoof.md", "content": "bad"}],
            "actor": {"actor_id": "attacker", "role": "owner"}
        }),
    )
    .await;
    assert!(!spoofed.ok);
    assert_eq!(spoofed.error.unwrap().code, "invalid_arguments");

    let mut config = RuntimeConfig::read_only(".", ".").without_builtin_tools();
    config.output_limit_bytes = 1;
    let registry = ToolRegistry::try_new_with_management(
        fixture.manager.current_snapshot().registry().clone(),
        &config,
        Some(context),
    )
    .unwrap();
    let result = registry
        .execute(
            "update_skill_draft",
            "update",
            json!({
                "revision_id": draft.revision_id,
                "files": [{"path": "references/safe.md", "content": "committed"}]
            }),
        )
        .await;

    assert!(result.ok, "{:?}", result.error);
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        PathBuf::from(record.storage_path)
            .join("references/safe.md")
            .is_file()
    );
}
