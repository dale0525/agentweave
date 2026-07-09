use mobile_ffi::{MobileInitConfig, MobileRuntime};
use tempfile::tempdir;

#[test]
fn initializes_runtime_and_returns_android_capabilities() {
    let dir = tempdir().unwrap();
    let runtime = MobileRuntime::initialize(MobileInitConfig {
        app_data_dir: dir.path().join("files").display().to_string(),
        cache_dir: dir.path().join("cache").display().to_string(),
        database_path: dir.path().join("general-agent.db").display().to_string(),
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
