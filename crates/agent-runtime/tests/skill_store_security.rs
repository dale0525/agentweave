use agent_runtime::skill_security::reject_reserved_skill_uri;
use agent_runtime::tools::{CommandMode, RuntimeConfig, ToolRegistry};
use agent_runtime::vfs::{AppDataVfs, VfsError};
use agent_runtime::{
    skill::SkillRegistry, skill_catalog::SkillCatalog, skill_manager::SkillManager,
};
use serde_json::json;
use std::path::Path;

const RESERVED_URIS: &[&str] = &[
    "app://builtin-skills/skill-bundle.json",
    "app://managed-skills/com.example.calendar/general-agent.json",
    "app://skill-staging/rev-1/SKILL.md",
    "app://skill-quarantine/rev-2/SKILL.md",
    "app://skill-state/database",
    "APP://BUILTIN-SKILLS/skill-bundle.json",
    "app://%62uiltin-skills/skill-bundle.json",
    "app://managed-skills%2fcom.example.calendar/general-agent.json",
];

#[test]
fn reserved_skill_uris_are_denied_before_normalization() {
    for uri in RESERVED_URIS {
        let error = reject_reserved_skill_uri(uri).unwrap_err().to_string();
        assert_eq!(
            error,
            "skill control-plane path is not available to generic tools"
        );
    }
    for uri in [
        "app://documents/builtin-skills.txt",
        "app://cache/managed-skills.json",
        "app://managed-skills-backup/file",
    ] {
        reject_reserved_skill_uri(uri).unwrap();
    }
}

#[test]
fn vfs_denies_reserved_roots_before_reporting_unsupported_root() {
    let vfs = AppDataVfs::new("/app/documents", "/app/cache");
    for uri in RESERVED_URIS {
        assert_eq!(
            vfs.resolve_uri(uri),
            Err(VfsError::ReservedSkillControlPath)
        );
    }
}

#[tokio::test]
async fn every_generic_filesystem_operation_denies_reserved_skill_uris() {
    let root = tempfile::tempdir().unwrap();
    let registry = ToolRegistry::new(
        SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty())
            .current_snapshot()
            .registry()
            .clone(),
        &RuntimeConfig::workspace_write(root.path(), root.path()),
    );

    let cases = [
        ("read_text_file", json!({"path": RESERVED_URIS[0]})),
        (
            "write_text_file",
            json!({"path": RESERVED_URIS[1], "text": "changed"}),
        ),
        ("list_directory", json!({"path": RESERVED_URIS[2]})),
        ("file_metadata", json!({"path": RESERVED_URIS[3]})),
        (
            "search_files",
            json!({"path": RESERVED_URIS[4], "pattern": "secret"}),
        ),
        (
            "apply_patch",
            json!({"patch": format!(
                "*** Begin Patch\n*** Add File: {}\n+changed\n*** End Patch\n",
                RESERVED_URIS[5]
            )}),
        ),
    ];

    for (index, (tool, arguments)) in cases.into_iter().enumerate() {
        let result = registry
            .execute(tool, &format!("reserved-{index}"), arguments)
            .await;
        assert!(!result.ok, "{tool} unexpectedly accepted a reserved URI");
        let error = result.error.unwrap();
        assert_eq!(error.code, "permission_denied");
        assert_eq!(
            error.message,
            "skill control-plane path is not available to generic tools"
        );
        assert!(
            !error
                .message
                .contains(root.path().to_string_lossy().as_ref())
        );
    }

    assert!(!Path::new(root.path()).join("app:").exists());
}

#[tokio::test]
async fn physical_control_roots_are_excluded_from_all_generic_filesystem_operations() {
    let root = tempfile::tempdir().unwrap();
    let control = root.path().join("private/managed-skills");
    tokio::fs::create_dir_all(&control).await.unwrap();
    tokio::fs::write(control.join("secret.txt"), "secret")
        .await
        .unwrap();
    let config = RuntimeConfig::workspace_write(root.path(), root.path())
        .excluding_workspace_roots([control.clone()]);
    let registry = ToolRegistry::new(SkillRegistry::empty(), &config);
    let cases = [
        (
            "read_text_file",
            json!({"path": "private/managed-skills/secret.txt"}),
        ),
        (
            "write_text_file",
            json!({"path": "private/managed-skills/new.txt", "text": "changed"}),
        ),
        ("list_directory", json!({"path": "private/managed-skills"})),
        (
            "file_metadata",
            json!({"path": "private/managed-skills/secret.txt"}),
        ),
        (
            "search_files",
            json!({"path": "private/managed-skills", "pattern": "secret"}),
        ),
        (
            "apply_patch",
            json!({"patch": "*** Begin Patch\n*** Add File: private/managed-skills/patched.txt\n+changed\n*** End Patch\n"}),
        ),
    ];
    for (index, (tool, arguments)) in cases.into_iter().enumerate() {
        let result = registry
            .execute(tool, &format!("physical-{index}"), arguments)
            .await;
        assert!(!result.ok, "{tool} unexpectedly accessed a control root");
        let error = result.error.unwrap();
        assert_eq!(error.code, "permission_denied");
        assert_eq!(
            error.message,
            "workspace path is reserved for skill management"
        );
        assert!(
            !error
                .message
                .contains(root.path().to_string_lossy().as_ref())
        );
    }
    let search = registry
        .execute(
            "search_files",
            "physical-parent-search",
            json!({"path": ".", "pattern": "secret"}),
        )
        .await;
    assert!(search.ok);
    assert!(
        search.data.unwrap()["matches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let listing = registry
        .execute(
            "list_directory",
            "physical-parent-list",
            json!({"path": "private"}),
        )
        .await;
    assert!(listing.ok);
    assert!(
        listing.data.unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!control.join("new.txt").exists());
    assert!(!control.join("patched.txt").exists());
}

#[tokio::test]
async fn command_is_absent_and_denied_when_any_control_root_is_configured() {
    let root = tempfile::tempdir().unwrap();
    let control = root.path().join("skill-state");
    std::fs::create_dir_all(&control).unwrap();
    std::fs::write(root.path().join("visible.txt"), "visible").unwrap();
    let config = RuntimeConfig::workspace_write(root.path(), root.path())
        .with_command_mode(CommandMode::Allowed)
        .excluding_workspace_roots([control]);
    let registry = ToolRegistry::try_new(SkillRegistry::empty(), &config).unwrap();

    assert!(
        !registry
            .definitions()
            .iter()
            .any(|tool| tool.name == "exec_command")
    );
    let command = registry
        .execute(
            "exec_command",
            "forced-command",
            json!({"cmd": "printf changed > changed.txt"}),
        )
        .await;
    assert!(!command.ok);
    assert_eq!(command.error.unwrap().code, "permission_denied");
    assert!(!root.path().join("changed.txt").exists());

    let read = registry
        .execute(
            "read_text_file",
            "filesystem-remains",
            json!({"path": "visible.txt"}),
        )
        .await;
    assert!(read.ok);
}

#[tokio::test]
async fn command_cannot_reach_an_absolute_control_root_outside_workspace() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    let config = RuntimeConfig::workspace_write(root.path(), root.path())
        .with_command_mode(CommandMode::Allowed)
        .excluding_workspace_roots([outside.path().to_path_buf()]);
    let registry = ToolRegistry::try_new(SkillRegistry::empty(), &config).unwrap();

    let result = registry
        .execute(
            "exec_command",
            "outside-command",
            json!({"cmd": format!("printf changed > {}", secret.display())}),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "permission_denied");
    assert_eq!(std::fs::read_to_string(secret).unwrap(), "secret");
}

#[cfg(unix)]
#[tokio::test]
async fn command_cannot_reach_a_control_root_through_workspace_symlink_alias() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    symlink(outside.path(), root.path().join("alias")).unwrap();
    let config = RuntimeConfig::workspace_write(root.path(), root.path())
        .with_command_mode(CommandMode::Allowed)
        .excluding_workspace_roots([outside.path().to_path_buf()]);
    let registry = ToolRegistry::try_new(SkillRegistry::empty(), &config).unwrap();

    let result = registry
        .execute(
            "exec_command",
            "alias-command",
            json!({"cmd": "printf changed > alias/secret.txt"}),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "permission_denied");
    assert_eq!(std::fs::read_to_string(secret).unwrap(), "secret");
}

#[tokio::test]
async fn development_command_without_control_roots_remains_available() {
    let root = tempfile::tempdir().unwrap();
    let config = RuntimeConfig::workspace_write(root.path(), root.path())
        .with_command_mode(CommandMode::Allowed);
    let registry = ToolRegistry::try_new(SkillRegistry::empty(), &config).unwrap();

    assert!(
        registry
            .definitions()
            .iter()
            .any(|tool| tool.name == "exec_command")
    );
    let result = registry
        .execute(
            "exec_command",
            "development-command",
            json!({"cmd": "printf available"}),
        )
        .await;

    assert!(result.ok);
    assert_eq!(result.data.unwrap()["stdout"], "available");
}
