use crate::{
    skill::{SkillExecutionContext, SkillRegistry},
    tools::ToolPermission,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[tokio::test]
async fn skill_manifest_tool_permission_defaults_to_read_workspace() {
    let root = unique_test_dir("default-tool-permission");
    write_skill_manifest(
        &root,
        "echo",
        json!({
            "name": "echo",
            "description": "Echo a text payload.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [
                {
                    "name": "echo",
                    "description": "Return text.",
                    "input_schema": { "type": "object" }
                }
            ]
        }),
    )
    .await;
    tokio::fs::write(
        root.join("echo").join("index.js"),
        "process.stdin.resume();\n",
    )
    .await
    .unwrap();

    let skill = SkillRegistry::load_development_skill(root.join("echo"))
        .await
        .unwrap();

    assert_eq!(
        skill.manifest.tools[0].permission,
        ToolPermission::ReadWorkspace
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn skill_manifest_tool_permission_can_be_write_workspace() {
    let root = unique_test_dir("write-tool-permission");
    write_skill_manifest(
        &root,
        "writer",
        json!({
            "name": "writer",
            "description": "Write workspace files.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [
                {
                    "name": "write_file",
                    "description": "Write a file.",
                    "permission": "write_workspace",
                    "input_schema": { "type": "object" }
                }
            ]
        }),
    )
    .await;
    tokio::fs::write(
        root.join("writer").join("index.js"),
        "process.stdin.resume();\n",
    )
    .await
    .unwrap();

    let skill = SkillRegistry::load_development_skill(root.join("writer"))
        .await
        .unwrap();

    assert_eq!(
        skill.manifest.tools[0].permission,
        ToolPermission::WriteWorkspace
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn skill_process_receives_workspace_environment() {
    let root = unique_test_dir("workspace-env");
    let workspace = unique_test_dir("workspace-env-root");
    tokio::fs::create_dir_all(&workspace).await.unwrap();
    write_skill_manifest(
        &root,
        "env",
        json!({
            "name": "env",
            "description": "Read runtime environment.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [
                {
                    "name": "read_env",
                    "description": "Read runtime environment.",
                    "input_schema": { "type": "object" }
                }
            ]
        }),
    )
    .await;
    tokio::fs::write(
        root.join("env").join("index.js"),
        "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ workspaceRoot: process.env.GENERAL_AGENT_WORKSPACE_ROOT, cwd: process.env.GENERAL_AGENT_CWD })));\n",
    )
    .await
    .unwrap();
    let registry = SkillRegistry::load_development(&root).await.unwrap();

    let result = registry
        .execute_with_context(
            "read_env",
            json!({}),
            SkillExecutionContext {
                workspace_root: workspace.clone(),
                cwd: workspace.clone(),
                output_limit_bytes: 8192,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result["workspaceRoot"].as_str(),
        Some(workspace.to_string_lossy().as_ref())
    );
    assert_eq!(
        result["cwd"].as_str(),
        Some(workspace.to_string_lossy().as_ref())
    );
    remove_test_dir(root).await;
    remove_test_dir(workspace).await;
}

#[tokio::test]
async fn skill_process_receives_called_tool_name() {
    let root = unique_test_dir("tool-name-env");
    write_skill_manifest(
        &root,
        "multi",
        json!({
            "name": "multi",
            "description": "Multiple runtime tools.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [
                { "name": "first_tool", "description": "First tool.", "input_schema": { "type": "object" } },
                { "name": "second_tool", "description": "Second tool.", "input_schema": { "type": "object" } }
            ]
        }),
    )
    .await;
    tokio::fs::write(
        root.join("multi").join("index.js"),
        "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ tool: process.env.GENERAL_AGENT_TOOL_NAME })));\n",
    )
    .await
    .unwrap();
    let registry = SkillRegistry::load_development(&root).await.unwrap();

    let result = registry
        .execute_with_context(
            "second_tool",
            json!({}),
            SkillExecutionContext {
                workspace_root: root.clone(),
                cwd: root.clone(),
                output_limit_bytes: 8192,
            },
        )
        .await
        .unwrap();

    assert_eq!(result["tool"].as_str(), Some("second_tool"));
    remove_test_dir(root).await;
}

async fn write_skill_manifest(root: &Path, folder: &str, manifest: Value) {
    let skill_dir = root.join(folder);
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(skill_dir.join("skill.json"), manifest.to_string())
        .await
        .unwrap();
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
}

async fn remove_test_dir(path: PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
