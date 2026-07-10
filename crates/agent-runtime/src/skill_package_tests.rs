use crate::skill_package::{
    DescriptorSource, SkillPackageDescriptor, SkillPackageId, SkillPackageKind,
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

async fn create_legacy_instruction_package(parent: &Path, folder: &str) -> PathBuf {
    let package = parent.join(folder);
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(package.join("SKILL.md"), "# Legacy skill")
        .await
        .unwrap();
    package
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
