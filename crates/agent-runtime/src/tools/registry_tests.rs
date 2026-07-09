use super::*;
use crate::skill::SkillRegistry;
use std::path::PathBuf;

#[test]
fn tool_definitions_include_source_and_schema_diagnostics() {
    let workspace_root = PathBuf::from("/workspace");
    let config = RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root);
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let diagnostics = registry.diagnostics();
    let create_directory = diagnostics
        .iter()
        .find(|tool| tool.name == "create_directory")
        .expect("create_directory diagnostic should exist");

    assert_eq!(create_directory.source, ToolSource::BuiltIn);
    assert_eq!(create_directory.permission, ToolPermission::WriteWorkspace);
    assert!(create_directory.schema.valid);
    assert_eq!(create_directory.namespace, None);
}

#[tokio::test]
async fn runtime_skill_diagnostics_include_skill_source() {
    let root = unique_test_dir("runtime-source-diagnostics");
    write_skill(
            &root,
            "echoer",
            "echoer_echo",
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    let registry = ToolRegistry::new(skills, &config);

    let diagnostic = registry
        .diagnostics()
        .into_iter()
        .find(|tool| tool.name == "echoer_echo")
        .unwrap();

    assert_eq!(
        diagnostic.source,
        ToolSource::RuntimeSkill {
            skill_name: "echoer".into()
        }
    );
    assert!(diagnostic.schema.valid);
    remove_test_dir(root).await;
}

#[test]
fn read_only_blocks_workspace_writes() {
    assert!(permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Disabled,
        ToolPermission::ReadWorkspace
    ));
    assert!(!permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Disabled,
        ToolPermission::WriteWorkspace
    ));
    assert!(permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Disabled,
        ToolPermission::ReadWorkspace
    ));
    assert!(permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Disabled,
        ToolPermission::WriteWorkspace
    ));
}

#[test]
fn command_permission_requires_workspace_write_and_command_allowed() {
    assert!(!permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Disabled,
        ToolPermission::ExecuteCommand
    ));
    assert!(!permission_allowed(
        RuntimeMode::ReadOnly,
        CommandMode::Allowed,
        ToolPermission::ExecuteCommand
    ));
    assert!(!permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Disabled,
        ToolPermission::ExecuteCommand
    ));
    assert!(permission_allowed(
        RuntimeMode::WorkspaceWrite,
        CommandMode::Allowed,
        ToolPermission::ExecuteCommand
    ));
}

#[test]
fn runtime_config_defaults_to_command_disabled() {
    let workspace_root = PathBuf::from("/workspace");
    let cwd = workspace_root.join("project");
    let workspace_write = RuntimeConfig::workspace_write(workspace_root.clone(), cwd.clone());
    let read_only = RuntimeConfig::read_only(workspace_root.clone(), cwd.clone());

    assert_eq!(workspace_write.workspace_root, workspace_root);
    assert_eq!(workspace_write.cwd, cwd);
    assert_eq!(workspace_write.mode, RuntimeMode::WorkspaceWrite);
    assert_eq!(workspace_write.command_mode, CommandMode::Disabled);
    assert!(workspace_write.built_in_tools_enabled);
    assert_eq!(workspace_write.max_tool_calls_per_turn, 16);
    assert_eq!(workspace_write.tool_timeout_ms, 30_000);
    assert_eq!(workspace_write.output_limit_bytes, 64 * 1024);
    assert_eq!(
        workspace_write.approval_policy,
        crate::policy::ApprovalPolicy::Never
    );
    assert_eq!(
        workspace_write.sandbox_profile.network,
        crate::policy::NetworkPolicy::UnrestrictedPlaceholder
    );

    assert_eq!(read_only.mode, RuntimeMode::ReadOnly);
    assert_eq!(read_only.command_mode, CommandMode::Disabled);
    assert_eq!(read_only.max_tool_calls_per_turn, 16);
    assert_eq!(read_only.tool_timeout_ms, 30_000);
    assert_eq!(read_only.output_limit_bytes, 64 * 1024);
}

#[test]
fn runtime_config_can_enable_development_command_mode() {
    let workspace_root = PathBuf::from("/workspace");
    let cwd = workspace_root.join("project");
    let config =
        RuntimeConfig::workspace_write(workspace_root, cwd).with_command_mode(CommandMode::Allowed);

    assert_eq!(config.command_mode, CommandMode::Allowed);
}

#[tokio::test]
async fn disabled_builtin_tools_leave_runtime_skills_model_visible() {
    let root = unique_test_dir("disabled-builtins");
    write_skill(
        &root,
        "echoer",
        "echoer_echo",
        "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
    )
    .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone()).without_builtin_tools();
    let registry = ToolRegistry::new(skills, &config);
    let definitions = registry.definitions();

    assert!(definitions.iter().any(|tool| tool.name == "echoer_echo"));
    assert!(
        !definitions
            .iter()
            .any(|tool| tool.name == "create_directory")
    );
    assert!(!definitions.iter().any(|tool| tool.name == "read_text_file"));
    assert!(!definitions.iter().any(|tool| tool.name == "apply_patch"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn enabled_builtin_tools_take_precedence_over_duplicate_runtime_skill_names() {
    let root = unique_test_dir("duplicate-builtin-skill-name");
    write_skill(
        &root,
        "filesystem",
        "read_text_file",
        "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ text: 'skill' })));\n",
    )
    .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    let registry = ToolRegistry::new(skills, &config);
    let definitions: Vec<_> = registry
        .definitions()
        .into_iter()
        .filter(|tool| tool.name == "read_text_file")
        .collect();

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].source, ToolSource::BuiltIn);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn disabled_builtin_tools_do_not_execute_builtin_names() {
    let root = unique_test_dir("disabled-builtins-execution");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone()).without_builtin_tools();
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute(
            "read_text_file",
            "call-1",
            serde_json::json!({ "path": "notes.txt" }),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "unknown_tool");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_skill_definitions_use_manifest_permissions() {
    let root = unique_test_dir("skill-permissions");
    write_skill_with_permission(
        &root,
        "writer",
        "write_file",
        "write_workspace",
        "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ ok: true })));\n",
    )
    .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig::workspace_write(root.clone(), root.clone()).without_builtin_tools();
    let registry = ToolRegistry::new(skills, &config);

    let definition = registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "write_file")
        .unwrap();

    assert_eq!(definition.permission, ToolPermission::WriteWorkspace);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn project_filesystem_skill_executes_when_builtin_tools_are_disabled() {
    let workspace = unique_test_dir("project-filesystem-skill");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    let skills = SkillRegistry::load_development(project_skills_root())
        .await
        .unwrap();
    let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
        .without_builtin_tools();
    let registry = ToolRegistry::new(skills, &config);
    let definition = registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "write_text_file")
        .unwrap();
    assert_eq!(
        definition.source,
        ToolSource::RuntimeSkill {
            skill_name: "filesystem".to_string()
        }
    );
    assert_eq!(definition.permission, ToolPermission::WriteWorkspace);

    let write_result = registry
        .execute(
            "write_text_file",
            "call-write",
            serde_json::json!({ "path": "notes.txt", "text": "hello\nneedle\n" }),
        )
        .await;
    assert!(write_result.ok);

    let read_result = registry
        .execute(
            "read_text_file",
            "call-read",
            serde_json::json!({ "path": "notes.txt" }),
        )
        .await;
    assert_eq!(read_result.data.unwrap()["text"], "hello\nneedle\n");

    let patch_result = registry
        .execute(
            "apply_patch",
            "call-patch",
            serde_json::json!({
                "patch": "*** Begin Patch\n*** Add File: patched.txt\n+patched\n*** End Patch\n"
            }),
        )
        .await;
    assert!(patch_result.ok);
    assert_eq!(
        tokio::fs::read_to_string(workspace.join("patched.txt"))
            .await
            .unwrap(),
        "patched\n"
    );
    remove_test_dir(workspace).await;
}

#[tokio::test]
async fn tool_registry_reports_approval_requirement_for_write_tools() {
    let root = unique_test_dir("approval-requirement");
    std::fs::create_dir_all(&root).unwrap();
    let mut config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    config.approval_policy = crate::policy::ApprovalPolicy::OnWorkspaceWrite;
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let requirement = registry.approval_requirement("create_directory").unwrap();

    assert_eq!(requirement.permission, ToolPermission::WriteWorkspace);
    assert_eq!(
        requirement.policy,
        crate::policy::ApprovalPolicy::OnWorkspaceWrite
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_includes_immediate_mcp_tools_with_namespaced_names() {
    let root = unique_test_dir("mcp-immediate");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "filesystem",
            "read_file",
            "Read a file through MCP.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Immediate,
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let tool = registry
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "mcp__filesystem__read_file")
        .unwrap();

    assert_eq!(tool.namespace.as_deref(), Some("mcp__filesystem"));
    assert_eq!(
        tool.source,
        ToolSource::Mcp {
            server: "filesystem".into()
        }
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_executes_static_mcp_adapter_result() {
    let root = unique_test_dir("mcp-static-exec");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![
            crate::tools::discovery::ExternalToolConfig::mcp(
                "clock",
                "now",
                "Return a static time.",
                serde_json::json!({ "type": "object" }),
                crate::tools::discovery::ExternalToolVisibility::Immediate,
            )
            .with_static_result(serde_json::json!({ "time": "12:00" })),
        ],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    let result = registry
        .execute("mcp__clock__now", "call-1", serde_json::json!({}))
        .await;

    assert!(result.ok);
    assert_eq!(result.data.unwrap()["time"], "12:00");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_rejects_namespaced_collisions() {
    let root = unique_test_dir("mcp-collision");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![
            crate::tools::discovery::ExternalToolConfig::mcp(
                "search",
                "lookup",
                "First lookup tool.",
                serde_json::json!({ "type": "object" }),
                crate::tools::discovery::ExternalToolVisibility::Immediate,
            ),
            crate::tools::discovery::ExternalToolConfig::mcp(
                "search",
                "lookup",
                "Second lookup tool.",
                serde_json::json!({ "type": "object" }),
                crate::tools::discovery::ExternalToolVisibility::Immediate,
            ),
        ],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };

    let result = ToolRegistry::try_new(SkillRegistry::empty_for_tests(), &config);

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("duplicate tool name")
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn deferred_mcp_tools_are_discoverable_but_not_model_visible() {
    let root = unique_test_dir("mcp-deferred");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "expensive_lookup",
            "Search a remote corpus.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Deferred {
                summary: "Remote corpus lookup.".into(),
            },
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    assert!(
        !registry
            .definitions()
            .iter()
            .any(|tool| tool.name == "mcp__search__expensive_lookup")
    );
    assert!(
        registry
            .discovery()
            .tools
            .iter()
            .any(|tool| tool.name == "mcp__search__expensive_lookup" && tool.deferred)
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn read_only_builtin_tools_are_parallel_safe() {
    let root = unique_test_dir("parallel-safe-read");
    std::fs::create_dir_all(&root).unwrap();
    let registry = ToolRegistry::new(
        SkillRegistry::empty_for_tests(),
        &RuntimeConfig::workspace_write(root.clone(), root.clone()),
    );

    assert!(registry.parallel_safe("read_text_file"));
    assert!(registry.parallel_safe("list_directory"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn write_command_runtime_and_external_tools_are_not_parallel_safe_by_default() {
    let root = unique_test_dir("parallel-unsafe");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "lookup",
            "Search.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Immediate,
        )],
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

    assert!(!registry.parallel_safe("create_directory"));
    assert!(!registry.parallel_safe("mcp__search__lookup"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_applies_skill_output_limit() {
    let root = unique_test_dir("registry-output-limit");
    write_skill(
            &root,
            "large",
            "large_output",
            "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ text: 'abcdef' })));\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        output_limit_bytes: 4,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute("large_output", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
    assert!(result.metadata.output_truncated);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_limits_skill_stdout_before_json_parsing() {
    let root = unique_test_dir("registry-skill-stdout-limit");
    write_skill(
            &root,
            "large-stdout",
            "large_stdout",
            "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write('x'.repeat(1024)));\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        output_limit_bytes: 4,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute("large_stdout", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
    assert!(result.metadata.output_truncated);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_applies_builtin_output_limit() {
    let root = unique_test_dir("registry-builtin-output-limit");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("big.txt"), "abcdef")
        .await
        .unwrap();
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        output_limit_bytes: 4,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute(
            "read_text_file",
            "call-1",
            serde_json::json!({ "path": "big.txt" }),
        )
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
    assert!(result.metadata.output_truncated);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_preserves_truncated_exec_command_success() {
    let root = unique_test_dir("registry-command-truncated-success");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        command_mode: CommandMode::Allowed,
        output_limit_bytes: 2048,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute(
            "exec_command",
            "call-1",
            serde_json::json!({ "cmd": "printf '%*s' 2048 '' | tr ' ' x" }),
        )
        .await;

    assert!(result.ok);
    assert!(result.metadata.stdout_truncated);
    assert!(result.metadata.output_truncated);
    assert!(serde_json::to_vec(&result).unwrap().len() <= config.output_limit_bytes);
    let data = result.data.unwrap();
    assert_eq!(data["terminated_by_runtime"], true);
    assert_eq!(data["exit_code"], Value::Null);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_applies_skill_timeout() {
    let root = unique_test_dir("registry-timeout");
    write_skill(
            &root,
            "slow",
            "slow_output",
            "const fs = require('fs');\nsetTimeout(() => fs.writeFileSync('timed-out.txt', 'late'), 100);\nprocess.stdin.resume();\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        tool_timeout_ms: 5,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute("slow_output", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "timeout");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !root.join("slow").join("timed-out.txt").exists(),
        "timed-out skill process should be stopped before it can write after timeout"
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_stops_skill_when_stdout_exceeds_limit() {
    let root = unique_test_dir("registry-stdout-limit");
    write_skill(
            &root,
            "chatty",
            "chatty_output",
            "process.stdin.resume();\nprocess.stdin.on('end', () => {\n  process.stdout.write('x'.repeat(1024));\n  setTimeout(() => {}, 1000);\n});\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        output_limit_bytes: 32,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute("chatty_output", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
    assert!(result.metadata.output_truncated);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn tool_registry_stops_skill_when_stderr_exceeds_limit() {
    let root = unique_test_dir("registry-stderr-limit");
    write_skill(
            &root,
            "chatty",
            "chatty_error",
            "process.stdin.resume();\nprocess.stdin.on('end', () => {\n  process.stderr.write('x'.repeat(1024));\n  setTimeout(() => {}, 1000);\n});\n",
        )
        .await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let config = RuntimeConfig {
        output_limit_bytes: 32,
        ..RuntimeConfig::workspace_write(root.clone(), root.clone())
    };
    let registry = ToolRegistry::new(skills, &config);

    let result = registry
        .execute("chatty_error", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
    assert!(result.metadata.output_truncated);
    remove_test_dir(root).await;
}

async fn write_skill(root: &std::path::Path, folder: &str, tool_name: &str, script: &str) {
    let skill_dir = root.join(folder);
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("skill.json"),
        serde_json::json!({
            "name": folder,
            "description": "Test runtime skill.",
            "version": "0.1.0",
            "entry": {
                "type": "command",
                "command": "node",
                "args": ["index.js"]
            },
            "tools": [
                {
                    "name": tool_name,
                    "description": "Test tool.",
                    "input_schema": { "type": "object" }
                }
            ]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(skill_dir.join("index.js"), script)
        .await
        .unwrap();
}

async fn write_skill_with_permission(
    root: &std::path::Path,
    folder: &str,
    tool_name: &str,
    permission: &str,
    script: &str,
) {
    let skill_dir = root.join(folder);
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("skill.json"),
        serde_json::json!({
            "name": folder,
            "description": "Test runtime skill.",
            "version": "0.1.0",
            "entry": {
                "type": "command",
                "command": "node",
                "args": ["index.js"]
            },
            "tools": [
                {
                    "name": tool_name,
                    "description": "Test tool.",
                    "permission": permission,
                    "input_schema": { "type": "object" }
                }
            ]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(skill_dir.join("index.js"), script)
        .await
        .unwrap();
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
}

fn project_skills_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("skills")
}

async fn remove_test_dir(path: PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
