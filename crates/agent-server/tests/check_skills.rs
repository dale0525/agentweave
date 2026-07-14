use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(unix)]
#[tokio::test]
async fn release_check_rejects_a_symlink_root_before_canonicalization() {
    use std::os::unix::fs::symlink;

    let root = unique_test_dir("check-root-symlink");
    let real = root.join("real");
    let linked = root.join("linked");
    tokio::fs::create_dir_all(&real).await.unwrap();
    symlink(&real, &linked).unwrap();

    let output = run_check(&["--root", linked.to_str().unwrap()]);
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.contains("skill root must be a real directory"),
        "{stderr}"
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn release_check_enforces_the_real_top_level_entry_limit() {
    let root = unique_test_dir("check-entry-limit");
    tokio::fs::create_dir_all(&root).await.unwrap();
    for index in 0..=agent_runtime::skill_store::DEFAULT_MAX_SKILL_ENTRIES {
        tokio::fs::write(root.join(format!("entry-{index:04}")), b"")
            .await
            .unwrap();
    }

    let output = run_check(&["--root", root.to_str().unwrap()]);
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("exceeds 4096 entry limit"), "{stderr}");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn repeated_roots_resolve_cross_root_packages_and_canonical_tools() {
    let root = unique_test_dir("check-roots");
    let first = root.join("first");
    let second = root.join("second");
    write_runtime_package(&first.join("tools"), "com.example.tools", "echo").await;
    write_instruction_package(
        &second.join("consumer"),
        "com.example.consumer",
        &["com.example.tools"],
        &["com.example.tools/echo"],
        &[],
    )
    .await;

    let output = run_check(&[
        "--root",
        first.to_str().unwrap(),
        "--root",
        second.to_str().unwrap(),
    ]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Skill release check passed: 2 package(s) across 2 root(s)\n"
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn release_check_aggregates_dependency_capability_connector_and_tool_errors() {
    let root = unique_test_dir("check-errors");
    write_instruction_package(
        &root.join("consumer"),
        "com.example.consumer",
        &["com.example.missing"],
        &["com.example.missing/read"],
        &["unknown.capability"],
    )
    .await;
    let descriptor_path = root.join("consumer/agentweave.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["requires"]["connectors"] = serde_json::json!(["missing-connector"]);
    tokio::fs::write(
        descriptor_path,
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();

    let output = run_check(&["--root", root.to_str().unwrap()]);
    let stderr = String::from_utf8(output.stderr).unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(stderr.contains("missing dependency: com.example.missing"));
    assert!(stderr.contains("unresolved capability: unknown.capability"));
    assert!(stderr.contains("unresolved connector: missing-connector"));
    assert!(stderr.contains("unresolved canonical runtime tool: com.example.missing/read"));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn release_check_aggregates_bad_siblings_and_cross_root_requirements() {
    let root = unique_test_dir("check-package-aggregation");
    let first = root.join("first");
    let second = root.join("second");
    tokio::fs::create_dir_all(first.join("bad-a"))
        .await
        .unwrap();
    tokio::fs::write(first.join("bad-a/agentweave.json"), "{")
        .await
        .unwrap();
    tokio::fs::create_dir_all(first.join("bad-b"))
        .await
        .unwrap();
    tokio::fs::write(first.join("bad-b/agentweave.json"), "[]")
        .await
        .unwrap();
    write_runtime_package(&second.join("provider"), "com.example.provider", "read").await;
    write_instruction_package(
        &first.join("consumer"),
        "com.example.consumer",
        &["com.example.provider", "com.example.missing"],
        &["com.example.provider/read"],
        &[],
    )
    .await;
    let args = [
        "--root",
        first.to_str().unwrap(),
        "--root",
        second.to_str().unwrap(),
    ];

    let first_run = run_check(&args);
    let second_run = run_check(&args);
    let stderr = String::from_utf8(first_run.stderr).unwrap();

    assert_eq!(first_run.status.code(), Some(1));
    assert_eq!(second_run.status.code(), Some(1));
    assert_eq!(stderr, String::from_utf8(second_run.stderr).unwrap());
    assert!(stderr.contains("bad-a"), "{stderr}");
    assert!(stderr.contains("bad-b"), "{stderr}");
    assert!(
        stderr.contains("missing dependency: com.example.missing"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("missing dependency: com.example.provider"),
        "{stderr}"
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn legacy_synthesis_is_a_warning_not_a_release_error() {
    let root = unique_test_dir("check-legacy");
    write_legacy_runtime_package(&root.join("legacy"), "echo").await;

    let output = run_check(&["--root", root.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("warning: legacy package descriptor synthesized"));
    assert!(stderr.contains("migration: synthesized package id legacy.local.legacy"));
    assert!(stderr.contains("inferred kind native_runtime"));
    assert!(stderr.contains("recommended agentweave.json"));
    assert!(stderr.contains("\"schemaVersion\":1"));
    assert!(!root.join("legacy/agentweave.json").exists());
    remove_test_dir(root).await;
}

#[test]
fn cli_rejects_missing_root_values_with_stable_error() {
    let output = run_check(&["--root"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "check-skills: --root requires a path\n"
    );
}

fn run_check(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_check-skills"))
        .args(args)
        .output()
        .unwrap()
}

async fn write_runtime_package(root: &Path, id: &str, tool: &str) {
    tokio::fs::create_dir_all(root).await.unwrap();
    write_descriptor(
        root,
        serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "0.1.0",
            "displayName": id,
            "kind": "native_runtime",
            "package": { "includeInstructions": false, "includeRuntime": true },
            "compatibility": { "platforms": ["desktop", "server"] },
            "requires": {
                "packages": [], "capabilities": ["shell.process"],
                "runtimeTools": [], "connectors": []
            }
        }),
    )
    .await;
    write_runtime_files(root, tool).await;
}

async fn write_legacy_runtime_package(root: &Path, tool: &str) {
    tokio::fs::create_dir_all(root).await.unwrap();
    write_runtime_files(root, tool).await;
}

async fn write_runtime_files(root: &Path, tool: &str) {
    let manifest = serde_json::json!({
        "name": "runtime",
        "description": "Runtime fixture.",
        "version": "0.1.0",
        "entry": { "type": "command", "command": "node", "args": ["index.js"] },
        "tools": [{
            "name": tool,
            "description": "Fixture tool.",
            "input_schema": { "type": "object" }
        }]
    });
    tokio::fs::write(
        root.join("skill.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("index.js"), "process.stdout.write('{}')\n")
        .await
        .unwrap();
}

async fn write_instruction_package(
    root: &Path,
    id: &str,
    dependencies: &[&str],
    runtime_tools: &[&str],
    capabilities: &[&str],
) {
    tokio::fs::create_dir_all(root).await.unwrap();
    let kind = if runtime_tools.is_empty() {
        "instruction_only"
    } else {
        "host_tools_only"
    };
    write_descriptor(
        root,
        serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "0.1.0",
            "displayName": id,
            "kind": kind,
            "package": { "includeInstructions": true, "includeRuntime": false },
            "compatibility": { "platforms": ["desktop", "server"] },
            "requires": {
                "packages": dependencies,
                "capabilities": capabilities,
                "runtimeTools": runtime_tools,
                "connectors": []
            }
        }),
    )
    .await;
    tokio::fs::write(
        root.join("SKILL.md"),
        "---\nname: fixture\ndescription: Fixture instructions.\n---\n\nUse tools.\n",
    )
    .await
    .unwrap();
}

async fn write_descriptor(root: &Path, value: serde_json::Value) {
    tokio::fs::write(
        root.join("agentweave.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .await
    .unwrap();
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agentweave-{name}-{}", uuid::Uuid::new_v4()))
}

async fn remove_test_dir(path: PathBuf) {
    let _ = tokio::fs::remove_dir_all(path).await;
}
