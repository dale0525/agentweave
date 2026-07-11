use mobile_ffi::{MobileInitConfig, MobileRuntime};
use tempfile::tempdir;

fn android_config(root: &std::path::Path) -> MobileInitConfig {
    let app_data_dir = root.join("files");
    MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: root.join("cache").display().to_string(),
        database_path: app_data_dir.join("general-agent.db").display().to_string(),
        skills_dir: "skills".into(),
        platform: "android".into(),
        capabilities: vec![
            "network.http".into(),
            "filesystem.app_data".into(),
            "secure_storage".into(),
            "model.http_provider".into(),
        ],
    }
}

fn write_instruction_package(
    skills_root: &std::path::Path,
    folder: &str,
    id: &str,
    name: &str,
    platform: &str,
) {
    let package_root = skills_root.join(folder);
    std::fs::create_dir_all(&package_root).unwrap();
    std::fs::write(
        package_root.join("general-agent.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "instruction_only",
            "package": {
                "includeInstructions": true,
                "includeRuntime": false
            },
            "compatibility": {
                "platforms": [platform]
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        package_root.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} instructions.\n---\n\n# {name}\n"),
    )
    .unwrap();
}

#[test]
fn initializes_runtime_and_returns_android_capabilities() {
    let dir = tempdir().unwrap();
    let runtime = MobileRuntime::initialize(android_config(dir.path())).unwrap();

    let diagnostics = runtime.diagnostics();
    assert_eq!(diagnostics.platform, "android");
    assert!(diagnostics.capabilities.contains(&"network.http".into()));
    assert!(diagnostics.database_ready);
}

#[test]
fn lists_instruction_only_skills_from_the_runtime_catalog() {
    let dir = tempdir().unwrap();
    let skill_dir = dir.path().join("files/skills/notes");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: notes\ndescription: Read app notes.\n---\n\n# Notes\n",
    )
    .unwrap();

    let runtime = MobileRuntime::initialize(android_config(dir.path())).unwrap();
    let skills = runtime.list_skills();

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].id, "notes");
    assert!(skills[0].available);
    assert_eq!(skills[0].reason, "Instruction skill loaded.");
}

#[test]
fn android_runtime_lists_only_platform_compatible_instruction_packages() {
    let dir = tempdir().unwrap();
    let skills_root = dir.path().join("files/skills");
    write_instruction_package(
        &skills_root,
        "android",
        "com.example.android",
        "android-only",
        "android",
    );
    write_instruction_package(
        &skills_root,
        "desktop",
        "com.example.desktop",
        "desktop-only",
        "desktop",
    );

    let runtime = MobileRuntime::initialize(android_config(dir.path())).unwrap();
    let skills = runtime.list_skills();

    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].id, "android-only");
}

#[test]
fn rejects_database_path_with_parent_dir_traversal() {
    let dir = tempdir().unwrap();
    let app_data_dir = dir.path().join("files");

    let error = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: app_data_dir.join("../escape.db").display().to_string(),
        skills_dir: "skills".into(),
        platform: "android".into(),
        capabilities: vec!["filesystem.app_data".into()],
    })
    .err()
    .expect("database path traversal should fail");

    assert!(error.to_string().contains("app-private"));
}

#[test]
fn rejects_skills_dir_with_parent_dir_traversal() {
    let dir = tempdir().unwrap();
    let app_data_dir = dir.path().join("files");

    let error = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: app_data_dir.join("general-agent.db").display().to_string(),
        skills_dir: "../skills".into(),
        platform: "android".into(),
        capabilities: vec!["filesystem.app_data".into()],
    })
    .err()
    .expect("skills path traversal should fail");

    assert!(error.to_string().contains("app-private"));
}

#[test]
fn rejects_absolute_skills_dir_outside_app_private_roots() {
    let dir = tempdir().unwrap();
    let outside_dir = tempdir().unwrap();
    let app_data_dir = dir.path().join("files");

    let error = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: app_data_dir.join("general-agent.db").display().to_string(),
        skills_dir: outside_dir.path().join("skills").display().to_string(),
        platform: "android".into(),
        capabilities: vec!["filesystem.app_data".into()],
    })
    .err()
    .expect("absolute skills dir outside app-private roots should fail");

    assert!(error.to_string().contains("app-private"));
}

#[cfg(unix)]
mod unix_symlink_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn rejects_symlink_escape_in_database_parent() {
        let dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let app_data_dir = dir.path().join("files");
        let escape_link = app_data_dir.join("escape");

        std::fs::create_dir_all(&app_data_dir).unwrap();
        symlink(outside_dir.path(), &escape_link).unwrap();

        let error = MobileRuntime::initialize(MobileInitConfig {
            app_data_dir: app_data_dir.display().to_string(),
            cache_dir: dir.path().join("cache").display().to_string(),
            database_path: escape_link.join("general-agent.db").display().to_string(),
            skills_dir: "skills".into(),
            platform: "android".into(),
            capabilities: vec!["filesystem.app_data".into()],
        })
        .err()
        .expect("database path via symlink escape should fail");

        assert!(error.to_string().contains("app-private"));
    }

    #[test]
    fn rejects_symlink_escape_in_skills_dir() {
        let dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let app_data_dir = dir.path().join("files");
        let escape_link = app_data_dir.join("escape");

        std::fs::create_dir_all(&app_data_dir).unwrap();
        symlink(outside_dir.path(), &escape_link).unwrap();

        let error = MobileRuntime::initialize(MobileInitConfig {
            app_data_dir: app_data_dir.display().to_string(),
            cache_dir: dir.path().join("cache").display().to_string(),
            database_path: app_data_dir.join("general-agent.db").display().to_string(),
            skills_dir: escape_link.join("skills").display().to_string(),
            platform: "android".into(),
            capabilities: vec!["filesystem.app_data".into()],
        })
        .err()
        .expect("skills path via symlink escape should fail");

        assert!(error.to_string().contains("app-private"));
    }
}
