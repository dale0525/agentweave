use crate::skill_package::{
    DescriptorSource, SkillCompatibility, SkillPackageDescriptor, SkillPackageId, SkillPackageKind,
    SkillPackageRequirements, SkillPackageTargets,
};
use semver::Version;
use serde_json::json;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn versioned_descriptor_value() -> serde_json::Value {
    json!({
        "schemaVersion": 1,
        "id": "com.example.calendar",
        "version": "1.2.0",
        "displayName": "Calendar",
        "kind": "instruction_only",
        "package": { "includeInstructions": true, "includeRuntime": false },
        "compatibility": { "minimumRuntimeVersion": "0.3.0", "platforms": ["desktop"] },
        "requires": { "packages": [], "capabilities": [], "runtimeTools": [], "connectors": [] }
    })
}

fn programmatic_descriptor() -> SkillPackageDescriptor {
    SkillPackageDescriptor {
        schema_version: 1,
        id: SkillPackageId::parse("com.example.programmatic").unwrap(),
        version: Version::new(1, 0, 0),
        display_name: "Programmatic".into(),
        kind: SkillPackageKind::InstructionOnly,
        package: SkillPackageTargets {
            include_instructions: true,
            include_runtime: false,
        },
        compatibility: SkillCompatibility::default(),
        requires: SkillPackageRequirements::default(),
    }
}

async fn create_legacy_instruction_package(parent: &Path, folder: &str) -> PathBuf {
    let package = parent.join(folder);
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(package.join("SKILL.md"), "# Legacy skill")
        .await
        .unwrap();
    package
}

async fn write_package_metadata(package: &Path, value: serde_json::Value) {
    tokio::fs::write(
        package.join("general-agent.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .await
    .unwrap();
}

#[test]
fn package_id_requires_reverse_domain_segments() {
    assert!(SkillPackageId::parse("com.example.calendar").is_ok());
    assert!(SkillPackageId::parse("calendar").is_err());
    assert!(SkillPackageId::parse("com..calendar").is_err());
    assert!(SkillPackageId::parse("Com.example.calendar").is_err());
}

#[test]
fn package_id_deserialization_reuses_parse_validation() {
    assert!(serde_json::from_str::<SkillPackageId>(r#""com.example.calendar""#).is_ok());
    assert!(serde_json::from_str::<SkillPackageId>(r#""Com.example.calendar""#).is_err());
}

#[test]
fn descriptor_deserialization_rejects_unsupported_schema_version() {
    let mut value = versioned_descriptor_value();
    value["schemaVersion"] = json!(2);

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_err());
}

#[test]
fn descriptor_deserialization_rejects_invalid_required_package_id() {
    let mut value = versioned_descriptor_value();
    value["requires"]["packages"] = json!(["calendar"]);

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_err());
}

#[test]
fn descriptor_deserialization_rejects_unknown_top_level_fields() {
    let mut value = versioned_descriptor_value();
    value["display_name"] = json!("Misspelled Calendar");

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_err());
}

#[test]
fn descriptor_deserialization_rejects_unknown_requirement_fields() {
    let mut value = versioned_descriptor_value();
    value["requires"]["runtimeTool"] = json!(["read_text_file"]);

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_err());
}

#[test]
fn descriptor_semantics_reject_invalid_kind_target_combinations() {
    let invalid_cases = [
        ("instruction_only", false, false, Vec::<&str>::new()),
        ("instruction_only", true, true, Vec::<&str>::new()),
        ("native_runtime", true, false, Vec::<&str>::new()),
        ("host_tools_only", true, true, vec!["read_text_file"]),
        ("host_tools_only", false, false, vec!["read_text_file"]),
    ];

    for (kind, include_instructions, include_runtime, runtime_tools) in invalid_cases {
        let mut value = versioned_descriptor_value();
        value["kind"] = json!(kind);
        value["package"]["includeInstructions"] = json!(include_instructions);
        value["package"]["includeRuntime"] = json!(include_runtime);
        value["requires"]["runtimeTools"] = json!(runtime_tools);

        assert!(
            serde_json::from_value::<SkillPackageDescriptor>(value).is_err(),
            "accepted invalid {kind} descriptor"
        );
    }
}

#[test]
fn descriptor_semantics_require_host_tool_dependencies() {
    let mut value = versioned_descriptor_value();
    value["kind"] = json!("host_tools_only");

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_err());
}

#[test]
fn descriptor_semantics_accept_valid_host_tools_only() {
    let mut value = versioned_descriptor_value();
    value["kind"] = json!("host_tools_only");
    value["requires"]["runtimeTools"] = json!(["read_text_file"]);

    assert!(serde_json::from_value::<SkillPackageDescriptor>(value).is_ok());
}

#[test]
fn programmatic_descriptor_validation_rejects_unsupported_schema() {
    let mut descriptor = programmatic_descriptor();
    descriptor.schema_version = 999;

    assert!(descriptor.validate().is_err());
}

#[test]
fn programmatic_descriptor_validation_rejects_invalid_kind_targets() {
    let mut descriptor = programmatic_descriptor();
    descriptor.kind = SkillPackageKind::NativeRuntime;

    assert!(descriptor.validate().is_err());
}

#[test]
fn programmatic_descriptor_validation_accepts_valid_descriptor() {
    assert!(programmatic_descriptor().validate().is_ok());
}

#[tokio::test]
async fn loads_versioned_package_descriptor() {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        r#"{
          "schemaVersion": 1,
          "id": "com.example.calendar",
          "version": "1.2.0",
          "displayName": "Calendar",
          "kind": "instruction_only",
          "package": { "includeInstructions": true, "includeRuntime": false },
          "compatibility": { "minimumRuntimeVersion": "0.3.0", "platforms": ["desktop"] },
          "requires": { "packages": [], "capabilities": [], "runtimeTools": [], "connectors": [] }
        }"#,
    )
    .await
    .unwrap();

    let loaded = SkillPackageDescriptor::load(root.path()).await.unwrap();

    assert_eq!(loaded.descriptor.id.as_str(), "com.example.calendar");
    assert_eq!(loaded.descriptor.version, Version::new(1, 2, 0));
    assert_eq!(loaded.descriptor.kind, SkillPackageKind::InstructionOnly);
    assert_eq!(loaded.source, DescriptorSource::Explicit);
    assert!(loaded.warnings.is_empty());
}

#[tokio::test]
async fn synthesizes_legacy_descriptor_with_warning() {
    let root = tempdir().unwrap();
    let package = root.path().join("echo");
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(
        package.join("skill.json"),
        r#"{
          "name":"echo","description":"Echo.","version":"0.1.0",
          "entry":{"type":"command","command":"node","args":["index.js"]},
          "tools":[{"name":"echo","description":"Echo.","input_schema":{"type":"object"}}]
        }"#,
    )
    .await
    .unwrap();

    let loaded = SkillPackageDescriptor::load(&package).await.unwrap();

    assert_eq!(loaded.descriptor.id.as_str(), "legacy.local.echo");
    assert_eq!(loaded.descriptor.kind, SkillPackageKind::NativeRuntime);
    assert_eq!(loaded.source, DescriptorSource::LegacySynthesized);
    assert_eq!(loaded.warnings.len(), 1);
}

#[tokio::test]
async fn loads_current_legacy_metadata_without_schema_version() {
    let root = tempdir().unwrap();
    let package = root.path().join("filesystem");
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(package.join("SKILL.md"), "# Filesystem")
        .await
        .unwrap();
    tokio::fs::write(
        package.join("skill.json"),
        r#"{"name":"filesystem","version":"0.4.0"}"#,
    )
    .await
    .unwrap();
    tokio::fs::write(
        package.join("general-agent.json"),
        r#"{
          "package": {
            "includeInstructions": true,
            "includeRuntime": true
          },
          "requires": {
            "packages": ["com.example.shared"],
            "capabilities": ["filesystem.read"],
            "runtimeTools": ["read_text_file", "write_text_file"],
            "connectors": ["storage"]
          }
        }"#,
    )
    .await
    .unwrap();

    let loaded = SkillPackageDescriptor::load(&package).await.unwrap();

    assert_eq!(loaded.source, DescriptorSource::LegacySynthesized);
    assert_eq!(loaded.descriptor.version, Version::new(0, 4, 0));
    assert!(loaded.descriptor.package.include_instructions);
    assert!(loaded.descriptor.package.include_runtime);
    assert_eq!(loaded.descriptor.requires.packages.len(), 1);
    assert_eq!(
        loaded.descriptor.requires.packages[0].as_str(),
        "com.example.shared"
    );
    assert_eq!(loaded.descriptor.requires.capabilities, ["filesystem.read"]);
    assert_eq!(
        loaded.descriptor.requires.runtime_tools,
        ["read_text_file", "write_text_file"]
    );
    assert_eq!(loaded.descriptor.requires.connectors, ["storage"]);
}

#[tokio::test]
async fn legacy_metadata_rejects_misspelled_schema_version() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "schema-typo").await;
    write_package_metadata(
        &package,
        json!({
            "schemaVerison": 1,
            "package": { "includeInstructions": true, "includeRuntime": false },
            "requires": {}
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_metadata_rejects_v1_fields_without_schema_version() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "mixed-v1").await;
    write_package_metadata(
        &package,
        json!({
            "id": "com.example.mixed",
            "version": "1.0.0",
            "displayName": "Mixed",
            "kind": "instruction_only",
            "package": { "includeInstructions": true, "includeRuntime": false },
            "requires": {}
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_metadata_rejects_unknown_package_fields() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "package-typo").await;
    write_package_metadata(
        &package,
        json!({
            "package": { "includeInstruction": true, "includeRuntime": false },
            "requires": {}
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_metadata_rejects_unknown_requirement_fields() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "requirements-typo").await;
    write_package_metadata(
        &package,
        json!({
            "package": { "includeInstructions": true, "includeRuntime": false },
            "requires": { "runtimeTool": ["read_text_file"] }
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_metadata_requires_complete_package_and_requires_sections() {
    let invalid_values = [
        json!({ "requires": {} }),
        json!({
            "package": { "includeInstructions": true, "includeRuntime": false }
        }),
        json!({
            "package": { "includeInstructions": true },
            "requires": {}
        }),
    ];

    for (index, value) in invalid_values.into_iter().enumerate() {
        let root = tempdir().unwrap();
        let package =
            create_legacy_instruction_package(root.path(), &format!("incomplete-{index}")).await;
        write_package_metadata(&package, value).await;

        assert!(
            SkillPackageDescriptor::load(&package).await.is_err(),
            "accepted incomplete legacy metadata case {index}"
        );
    }
}

#[tokio::test]
async fn legacy_metadata_rejects_missing_declared_instructions() {
    let root = tempdir().unwrap();
    let package = root.path().join("missing-instructions");
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(
        package.join("skill.json"),
        r#"{"name":"runtime","version":"0.1.0"}"#,
    )
    .await
    .unwrap();
    write_package_metadata(
        &package,
        json!({
            "package": { "includeInstructions": true, "includeRuntime": true },
            "requires": {}
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_metadata_rejects_missing_declared_runtime() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "missing-runtime").await;
    write_package_metadata(
        &package,
        json!({
            "package": { "includeInstructions": true, "includeRuntime": true },
            "requires": {}
        }),
    )
    .await;

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn legacy_host_tool_requirements_determine_kind() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "host-tools").await;
    write_package_metadata(
        &package,
        json!({
            "package": { "includeInstructions": true, "includeRuntime": false },
            "requires": { "runtimeTools": ["read_text_file"] }
        }),
    )
    .await;

    let loaded = SkillPackageDescriptor::load(&package).await.unwrap();

    assert_eq!(loaded.descriptor.kind, SkillPackageKind::HostToolsOnly);
}

#[tokio::test]
async fn lossy_legacy_folder_names_include_stable_hashes() {
    let root = tempdir().unwrap();
    let underscored = create_legacy_instruction_package(root.path(), "my_skill").await;
    let hyphenated = create_legacy_instruction_package(root.path(), "my-skill").await;

    let first = SkillPackageDescriptor::load(&underscored).await.unwrap();
    let repeated = SkillPackageDescriptor::load(&underscored).await.unwrap();
    let lossless = SkillPackageDescriptor::load(&hyphenated).await.unwrap();

    assert_eq!(first.descriptor.id, repeated.descriptor.id);
    assert_ne!(first.descriptor.id, lossless.descriptor.id);
}

#[tokio::test]
async fn lossy_legacy_ids_use_a_namespace_disjoint_from_lossless_ids() {
    let root = tempdir().unwrap();
    let lossy_package = create_legacy_instruction_package(root.path(), "my_skill").await;
    let collision_package =
        create_legacy_instruction_package(root.path(), "my-skill-e6696e81e346").await;

    let lossy = SkillPackageDescriptor::load(&lossy_package).await.unwrap();
    let lossless = SkillPackageDescriptor::load(&collision_package)
        .await
        .unwrap();

    assert_eq!(
        lossy.descriptor.id.as_str(),
        "legacy.lossy.my-skill-e6696e81e346"
    );
    assert_eq!(
        lossless.descriptor.id.as_str(),
        "legacy.local.my-skill-e6696e81e346"
    );
    assert_ne!(lossy.descriptor.id, lossless.descriptor.id);
}

#[tokio::test]
async fn non_ascii_legacy_folder_names_produce_distinct_valid_ids() {
    let root = tempdir().unwrap();
    let first_package = create_legacy_instruction_package(root.path(), "\u{6280}\u{80fd}").await;
    let second_package = create_legacy_instruction_package(root.path(), "\u{65e5}\u{5386}").await;

    let first = SkillPackageDescriptor::load(&first_package).await.unwrap();
    let second = SkillPackageDescriptor::load(&second_package).await.unwrap();

    assert_ne!(first.descriptor.id, second.descriptor.id);
    assert!(SkillPackageId::parse(first.descriptor.id.as_str()).is_ok());
    assert!(SkillPackageId::parse(second.descriptor.id.as_str()).is_ok());
}

#[tokio::test]
async fn long_legacy_folder_names_produce_bounded_valid_ids() {
    let root = tempdir().unwrap();
    let folder = "a".repeat(200);
    let package = create_legacy_instruction_package(root.path(), &folder).await;

    let loaded = SkillPackageDescriptor::load(&package).await.unwrap();

    assert!(loaded.descriptor.id.as_str().len() <= 128);
    assert!(SkillPackageId::parse(loaded.descriptor.id.as_str()).is_ok());
}

#[tokio::test]
async fn missing_package_root_is_rejected() {
    let root = tempdir().unwrap();
    let missing = root.path().join("missing");

    assert!(SkillPackageDescriptor::load(&missing).await.is_err());
}

#[tokio::test]
async fn file_package_root_is_rejected() {
    let root = tempdir().unwrap();
    let package_file = root.path().join("package");
    tokio::fs::write(&package_file, "not a directory")
        .await
        .unwrap();

    assert!(SkillPackageDescriptor::load(&package_file).await.is_err());
}

#[tokio::test]
async fn empty_legacy_package_root_is_rejected() {
    let root = tempdir().unwrap();
    let package = root.path().join("empty");
    tokio::fs::create_dir_all(&package).await.unwrap();

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[tokio::test]
async fn descriptor_filesystem_errors_are_propagated() {
    let root = tempdir().unwrap();
    let package = create_legacy_instruction_package(root.path(), "broken-metadata").await;
    tokio::fs::create_dir(package.join("general-agent.json"))
        .await
        .unwrap();

    assert!(SkillPackageDescriptor::load(&package).await.is_err());
}

#[cfg(unix)]
mod unix_symlink_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn symlink_package_root_is_rejected() {
        let links = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_package =
            create_legacy_instruction_package(outside.path(), "outside-package").await;
        let package_link = links.path().join("linked-package");
        symlink(&outside_package, &package_link).unwrap();

        assert!(SkillPackageDescriptor::load(&package_link).await.is_err());
    }

    #[tokio::test]
    async fn symlink_general_agent_metadata_is_rejected() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let package = create_legacy_instruction_package(root.path(), "linked-metadata").await;
        let outside_metadata = outside.path().join("general-agent.json");
        tokio::fs::write(
            &outside_metadata,
            r#"{"package":{"includeInstructions":true,"includeRuntime":false},"requires":{}}"#,
        )
        .await
        .unwrap();
        symlink(&outside_metadata, package.join("general-agent.json")).unwrap();

        assert!(SkillPackageDescriptor::load(&package).await.is_err());
    }

    #[tokio::test]
    async fn symlink_skill_manifest_is_rejected() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let package = root.path().join("linked-runtime");
        tokio::fs::create_dir_all(&package).await.unwrap();
        let outside_manifest = outside.path().join("skill.json");
        tokio::fs::write(&outside_manifest, r#"{"name":"linked","version":"0.1.0"}"#)
            .await
            .unwrap();
        symlink(&outside_manifest, package.join("skill.json")).unwrap();

        assert!(SkillPackageDescriptor::load(&package).await.is_err());
    }

    #[tokio::test]
    async fn symlink_skill_instructions_are_rejected() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let package = root.path().join("linked-instructions");
        tokio::fs::create_dir_all(&package).await.unwrap();
        let outside_instructions = outside.path().join("SKILL.md");
        tokio::fs::write(&outside_instructions, "# Outside")
            .await
            .unwrap();
        symlink(&outside_instructions, package.join("SKILL.md")).unwrap();

        assert!(SkillPackageDescriptor::load(&package).await.is_err());
    }
}
