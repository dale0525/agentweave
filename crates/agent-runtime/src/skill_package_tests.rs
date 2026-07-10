use crate::skill_package::{
    DescriptorSource, SkillPackageDescriptor, SkillPackageId, SkillPackageKind,
};
use semver::Version;
use tempfile::tempdir;

#[test]
fn package_id_requires_reverse_domain_segments() {
    assert!(SkillPackageId::parse("com.example.calendar").is_ok());
    assert!(SkillPackageId::parse("calendar").is_err());
    assert!(SkillPackageId::parse("com..calendar").is_err());
    assert!(SkillPackageId::parse("Com.example.calendar").is_err());
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
