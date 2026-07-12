use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService, SkillDraftValidation,
    SkillManagementError,
};
use crate::skill_management_tools::{SkillManagementToolContext, SkillManagementTools};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use crate::skill_source::{DirectorySkillSource, ManagedSkillSource, SkillLayer};
use crate::skill_state::{SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use crate::storage::Storage;
use crate::tools::{RuntimeConfig, ToolRegistry};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};

pub(crate) struct AuthoringFixture {
    _app: TempDir,
    _cache: TempDir,
    _builtin: Option<TempDir>,
    pub(crate) imports: TempDir,
    pub(crate) exports: TempDir,
    pub(crate) state: SkillStateStore,
    pub(crate) store: SkillRevisionStore,
    pub(crate) manager: SkillManager,
    pub(crate) service: OwnerSkillManagementService,
}

impl AuthoringFixture {
    pub(crate) async fn new() -> Self {
        Self::with_policy(SkillManagementPolicy::owner_only()).await
    }

    pub(crate) async fn with_connectors(
        connectors: impl IntoIterator<Item = &'static str>,
    ) -> Self {
        let mut fixture = Self::new().await;
        fixture.service = fixture.service.clone().with_connector_catalog(connectors);
        fixture
    }

    async fn with_known_runtime_tool() -> Self {
        Self::with_known_runtime_tool_and_policy(SkillManagementPolicy::owner_only()).await
    }

    async fn with_known_runtime_tool_and_policy(policy: SkillManagementPolicy) -> Self {
        let builtin = tempdir().unwrap();
        let package_root = builtin.path().join("host-runtime");
        tokio::fs::create_dir_all(&package_root).await.unwrap();
        tokio::fs::write(
            package_root.join("general-agent.json"),
            serde_json::json!({
                "schemaVersion": 1,
                "id": "com.example.host-runtime",
                "version": "1.0.0",
                "displayName": "Host runtime",
                "kind": "native_runtime",
                "package": {"includeInstructions": false, "includeRuntime": true}
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            package_root.join("skill.json"),
            serde_json::json!({
                "name": "host-runtime",
                "description": "Known host tools.",
                "version": "1.0.0",
                "entry": {"type": "command", "command": "false", "args": []},
                "tools": [{
                    "name": "calendar_create",
                    "description": "Create calendar events.",
                    "input_schema": {"type": "object"}
                }]
            })
            .to_string(),
        )
        .await
        .unwrap();
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
            sources: vec![
                Arc::new(DirectorySkillSource::new(
                    SkillLayer::Builtin,
                    builtin.path(),
                )),
                Arc::new(ManagedSkillSource::from_store(store.clone())),
            ],
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
            _builtin: Some(builtin),
            imports,
            exports,
            state,
            store,
            manager,
            service,
        }
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
            _builtin: None,
            imports,
            exports,
            state,
            store,
            manager,
            service,
        }
    }

    pub(crate) fn actor(&self, grants: impl IntoIterator<Item = SkillGrant>) -> ActorContext {
        ActorContext::owner("owner-1", grants)
    }

    pub(crate) async fn draft(&self) -> crate::skill_management::SkillDraftSummary {
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

pub(crate) async fn write_package(root: &std::path::Path, id: &str, kind: SkillPackageKind) {
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

pub(crate) fn update(path: &str, content: impl Into<String>) -> DraftFileUpdate {
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
async fn known_host_tool_draft_creates_and_validates_through_production_registry() {
    let fixture = AuthoringFixture::with_known_runtime_tool().await;
    let draft = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.calendar-host").unwrap(),
                display_name: "Calendar host".into(),
                description: "Create calendar events.".into(),
                kind: SkillPackageKind::HostToolsOnly,
                required_tools: vec!["calendar_create".into()],
            },
        )
        .await
        .unwrap();
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();

    assert!(validation.ok, "{:?}", validation.errors);
    assert_eq!(validation.required_tools, ["calendar_create"]);
    assert_eq!(validation.resolver_status, "active");
    assert_eq!(
        fixture
            .state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn builtin_override_requires_allowlist_and_override_grant_for_all_protection_states() {
    let package_id = SkillPackageId::parse("com.example.host-runtime").unwrap();
    for protected in [false, true] {
        for allowlisted in [false, true] {
            for granted in [false, true] {
                let mut policy = SkillManagementPolicy::owner_only();
                if protected {
                    policy = policy.protect(package_id.clone());
                }
                if allowlisted {
                    policy = policy.allow_override(package_id.clone());
                }
                let fixture = AuthoringFixture::with_known_runtime_tool_and_policy(policy).await;
                let draft = fixture
                    .service
                    .create_draft(
                        &fixture.actor([SkillGrant::CreateDraft]),
                        CreateSkillDraftRequest {
                            package_id: package_id.clone(),
                            display_name: "Managed override".into(),
                            description: "Override the builtin instructions.".into(),
                            kind: SkillPackageKind::InstructionOnly,
                            required_tools: Vec::new(),
                        },
                    )
                    .await
                    .unwrap();
                let mut grants = vec![SkillGrant::Validate];
                if granted {
                    grants.push(SkillGrant::OverrideBuiltin);
                }
                let result = fixture
                    .service
                    .validate_draft(&fixture.actor(grants), &draft.revision_id)
                    .await;
                if allowlisted && granted {
                    let validation = result.unwrap();
                    assert!(validation.ok, "{:?}", validation.errors);
                    assert_eq!(validation.resolver_status, "active");
                } else {
                    let error = result.unwrap_err();
                    assert!(matches!(
                        error.downcast_ref::<SkillManagementError>(),
                        Some(SkillManagementError::Denied {
                            operation: "override_builtin"
                        })
                    ));
                    assert_eq!(
                        fixture
                            .state
                            .revision_validation(&draft.revision_id)
                            .await
                            .unwrap(),
                        json!({"status": "pending"})
                    );
                }
            }
        }
    }
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
        json!({
            "addedCapabilities": [],
            "addedConnectors": [],
            "addedTools": [],
            "removedCapabilities": [],
            "removedConnectors": [],
            "removedTools": []
        })
    );
}

#[tokio::test]
async fn validation_reports_catalog_dependency_capability_and_protected_policy_errors() {
    let fixture = AuthoringFixture::new().await;
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
            "addedConnectors": [],
            "addedTools": [],
            "removedCapabilities": [],
            "removedConnectors": [],
            "removedTools": []
        })
    );
    let joined = validation.errors.join("\n");
    assert!(joined.contains("catalog"), "{joined}");
    assert!(joined.contains("missing dependency"), "{joined}");
    assert!(joined.contains("missing capability"), "{joined}");
    assert!(validation.errors.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[tokio::test]
async fn protected_validation_denial_preserves_pending_validation() {
    let package_id = SkillPackageId::parse("com.example.calendar").unwrap();
    let fixture =
        AuthoringFixture::with_policy(SkillManagementPolicy::owner_only().protect(package_id))
            .await;
    let draft = fixture.draft().await;
    let before = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();

    let error = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("override_builtin denied"));
    assert_eq!(
        fixture
            .state
            .revision_validation(&draft.revision_id)
            .await
            .unwrap(),
        before
    );
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
async fn draft_test_rejects_stale_snapshot_generation() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    fixture.manager.reload().await.unwrap();

    let error = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), &draft.revision_id)
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    assert_eq!(fixture.manager.current_snapshot().generation(), 2);
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
