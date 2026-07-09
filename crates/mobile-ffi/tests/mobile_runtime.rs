use mobile_ffi::{MobileInitConfig, MobileRuntime};
use tempfile::tempdir;

#[test]
fn initializes_runtime_and_returns_android_capabilities() {
    let dir = tempdir().unwrap();
    let app_data_dir = dir.path().join("files");
    let cache_dir = dir.path().join("cache");
    let runtime = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: cache_dir.display().to_string(),
        database_path: app_data_dir
            .join("general-agent.db")
            .display()
            .to_string(),
        skills_dir: "skills".into(),
        platform: "android".into(),
        capabilities: vec![
            "network.http".into(),
            "filesystem.app_data".into(),
            "secure_storage".into(),
            "model.http_provider".into(),
        ],
    })
    .unwrap();

    let diagnostics = runtime.diagnostics();
    assert_eq!(diagnostics.platform, "android");
    assert!(diagnostics.capabilities.contains(&"network.http".into()));
    assert!(diagnostics.database_ready);
}

#[test]
fn rejects_database_path_with_parent_dir_traversal() {
    let dir = tempdir().unwrap();
    let app_data_dir = dir.path().join("files");

    let error = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: app_data_dir.display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: app_data_dir
            .join("../escape.db")
            .display()
            .to_string(),
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
        database_path: app_data_dir
            .join("general-agent.db")
            .display()
            .to_string(),
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
        database_path: app_data_dir
            .join("general-agent.db")
            .display()
            .to_string(),
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
            database_path: app_data_dir
                .join("general-agent.db")
                .display()
                .to_string(),
            skills_dir: escape_link.join("skills").display().to_string(),
            platform: "android".into(),
            capabilities: vec!["filesystem.app_data".into()],
        })
        .err()
        .expect("skills path via symlink escape should fail");

        assert!(error.to_string().contains("app-private"));
    }
}
