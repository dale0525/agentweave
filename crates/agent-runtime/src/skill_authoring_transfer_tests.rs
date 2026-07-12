use crate::skill_authoring_tests::{AuthoringFixture, write_package};
use crate::skill_package::SkillPackageKind;
use crate::skill_policy::SkillGrant;
use crate::skill_state::{SkillLayerRecord, SkillRevisionStatus};

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
async fn concurrent_same_source_import_replays_one_quarantined_revision() {
    let fixture = AuthoringFixture::new().await;
    write_package(
        &fixture.imports.path().join("same-source"),
        "com.example.same-source",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let actor = fixture.actor([SkillGrant::Import]);

    let (left, right) = tokio::join!(
        fixture
            .service
            .import_draft(&actor, std::path::Path::new("same-source")),
        fixture
            .service
            .import_draft(&actor, std::path::Path::new("same-source")),
    );
    let left = left.unwrap();
    let right = right.unwrap();

    assert_eq!(left.revision_id, right.revision_id);
    assert_eq!(left.status, "quarantined");
    assert_eq!(
        fixture
            .state
            .get_revision(&left.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Quarantined
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
        assert!(matches!(
            linked.downcast_ref::<crate::skill_management::SkillManagementError>(),
            Some(crate::skill_management::SkillManagementError::InvalidRequest(_))
        ));
        assert!(!linked.to_string().contains("linked"));

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
        assert!(matches!(
            hard_linked.downcast_ref::<crate::skill_management::SkillManagementError>(),
            Some(crate::skill_management::SkillManagementError::InvalidRequest(_))
        ));
        assert!(!hard_linked.to_string().contains("hard-linked"));
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
async fn connector_only_validation_uses_host_catalog_and_reports_permission_diff() {
    let fixture = AuthoringFixture::with_connectors(["com.example.calendar"]).await;
    let package_root = fixture.imports.path().join("connector-only");
    write_package(
        &package_root,
        "com.example.connector-only",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = package_root.join("general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["connectors"] = serde_json::json!(["com.example.calendar"]);
    tokio::fs::write(
        descriptor_path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
    )
    .await
    .unwrap();

    let imported = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            std::path::Path::new("connector-only"),
        )
        .await
        .unwrap();
    let validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();

    assert!(validation.ok, "{:?}", validation.errors);
    assert_eq!(validation.required_connectors, ["com.example.calendar"]);
    assert_eq!(
        validation.permission_diff,
        serde_json::json!({
            "addedCapabilities": [],
            "addedConnectors": ["com.example.calendar"],
            "addedTools": [],
            "removedCapabilities": [],
            "removedConnectors": [],
            "removedTools": []
        })
    );

    let approval = fixture
        .service
        .request_activation(
            &fixture.actor([SkillGrant::Activate]),
            &imported.revision_id,
        )
        .await
        .unwrap();
    fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &crate::skill_policy::ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap();
    let replacement = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            crate::skill_management::CreateSkillDraftRequest {
                package_id: crate::skill_package::SkillPackageId::parse(
                    "com.example.connector-only",
                )
                .unwrap(),
                display_name: "Connector removed".into(),
                description: "No connector is required.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    let replacement_validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &replacement.revision_id,
        )
        .await
        .unwrap();
    assert_eq!(
        replacement_validation.permission_diff["removedConnectors"],
        serde_json::json!(["com.example.calendar"])
    );
}

#[tokio::test]
async fn connector_only_validation_rejects_unknown_connector() {
    let fixture = AuthoringFixture::new().await;
    let package_root = fixture.imports.path().join("unknown-connector");
    write_package(
        &package_root,
        "com.example.unknown-connector",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = package_root.join("general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["connectors"] = serde_json::json!(["com.example.missing"]);
    tokio::fs::write(
        descriptor_path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
    )
    .await
    .unwrap();
    let imported = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            std::path::Path::new("unknown-connector"),
        )
        .await
        .unwrap();

    let validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();

    assert!(!validation.ok);
    assert!(
        validation
            .errors
            .iter()
            .any(|error| error == "unknown required connector: com.example.missing")
    );
    assert_eq!(validation.required_connectors, ["com.example.missing"]);
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

    let replay = fixture
        .service
        .export_managed_skill(
            &fixture.actor([SkillGrant::Export]),
            &draft.package_id,
            std::path::Path::new("calendar"),
        )
        .await
        .unwrap();
    assert_eq!(replay, exported);
    write_package(
        &fixture.exports.path().join("different"),
        "com.example.different-export",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let conflict = fixture
        .service
        .export_managed_skill(
            &fixture.actor([SkillGrant::Export]),
            &draft.package_id,
            std::path::Path::new("different"),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        conflict.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    assert!(
        fixture
            .service
            .export_managed_skill(
                &fixture.actor([SkillGrant::Export]),
                &draft.package_id,
                std::path::Path::new("../escape"),
            )
            .await
            .is_err()
    );
}
