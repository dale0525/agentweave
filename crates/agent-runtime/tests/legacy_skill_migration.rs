use agent_runtime::skill_migration::scan_legacy_packages;
use agent_runtime::skill_package::SkillPackageKind;

#[tokio::test]
async fn legacy_scan_reports_recommended_descriptor_without_rewriting_source() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("Legacy Calendar");
    tokio::fs::create_dir(&package).await.unwrap();
    tokio::fs::write(
        package.join("SKILL.md"),
        "---\nname: legacy-calendar\ndescription: Legacy calendar.\n---\n",
    )
    .await
    .unwrap();
    let before = tokio::fs::read(package.join("SKILL.md")).await.unwrap();

    let diagnostics = scan_legacy_packages(root.path()).await.unwrap();

    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert_eq!(diagnostic.package_path, package);
    assert_eq!(diagnostic.inferred_kind, SkillPackageKind::InstructionOnly);
    assert_eq!(
        diagnostic.synthesized_package_id,
        diagnostic.recommended_descriptor.id
    );
    assert_eq!(
        tokio::fs::read(diagnostic.package_path.join("SKILL.md"))
            .await
            .unwrap(),
        before
    );
    assert!(!diagnostic.package_path.join("agentweave.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn legacy_scan_is_bounded_and_does_not_follow_package_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    tokio::fs::write(outside.path().join("SKILL.md"), "# Outside\n")
        .await
        .unwrap();
    symlink(outside.path(), root.path().join("linked-package")).unwrap();

    let diagnostics = scan_legacy_packages(root.path()).await.unwrap();

    assert!(diagnostics.is_empty());
    assert!(!outside.path().join("agentweave.json").exists());
}
