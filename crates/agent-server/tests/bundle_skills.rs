use agent_runtime::{
    platform::{CapabilitySet, PlatformId},
    skill_bundle::{
        BundleSkillSource, SKILL_BUNDLE_CURRENT_FILE, SKILL_BUNDLE_GENERATIONS_DIR,
        SKILL_BUNDLE_LOCK_FILE, SKILL_BUNDLE_MANIFEST_FILE,
    },
    skill_manager::{SkillManager, SkillManagerConfig},
    skill_source::SkillSource,
};
use semver::Version;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

#[test]
fn cli_reports_stable_required_and_duplicate_argument_errors() {
    assert_cli_error(&[], "bundle-skills: missing required --source <path>\n");
    assert_cli_error(
        &["--source", "skills"],
        "bundle-skills: missing required --output <path>\n",
    );
    assert_cli_error(
        &["--source", "skills", "--output", "dist"],
        "bundle-skills: missing required --platform <desktop|server|android|ios|web>\n",
    );
    assert_cli_error(
        &[
            "--source",
            "skills",
            "--output",
            "one",
            "--output",
            "two",
            "--platform",
            "desktop",
        ],
        "bundle-skills: --output may be provided only once\n",
    );
    assert_cli_error(&["--wat"], "bundle-skills: unknown argument: --wat\n");
}

#[test]
fn cli_reports_stable_missing_value_and_platform_errors() {
    assert_cli_error(&["--source"], "bundle-skills: --source requires a path\n");
    assert_cli_error(
        &[
            "--source",
            "skills",
            "--output",
            "dist",
            "--platform",
            "console",
        ],
        "bundle-skills: unsupported platform: console\n",
    );
}

#[tokio::test]
async fn cli_builds_exact_verified_package_and_canonical_runtime_tool_loads() {
    let root = unique_test_dir("bundle-cli");
    let source = root.join("source");
    let output = root.join("bundle");
    write_runtime_package(&source.join("echo"), "com.example.echo").await;

    let result = run_cli(&[
        "--source",
        source.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--platform",
        "desktop",
    ]);

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("bundled 1 package(s) into {}\n", output.display())
    );
    assert!(output.join(SKILL_BUNDLE_CURRENT_FILE).is_file());
    let generation = active_generation(&output).await;
    assert!(generation.join(SKILL_BUNDLE_MANIFEST_FILE).is_file());
    assert!(generation.join(SKILL_BUNDLE_LOCK_FILE).is_file());
    assert!(
        generation
            .join("com.example.echo/general-agent.json")
            .is_file()
    );
    assert!(generation.join("com.example.echo/skill.json").is_file());
    assert!(generation.join("com.example.echo/index.js").is_file());
    let source = Arc::new(BundleSkillSource::open(&output).await.unwrap());
    assert_eq!(
        source.layer(),
        agent_runtime::skill_source::SkillLayer::Builtin
    );
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![source],
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::desktop_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 1, 0),
    })
    .await
    .unwrap();
    let result = manager
        .current_snapshot()
        .registry()
        .execute(
            "com.example.echo/echo",
            serde_json::json!({ "text": "hello" }),
        )
        .await
        .unwrap();
    assert_eq!(result, serde_json::json!({ "text": "hello" }));
    remove_test_dir(root).await;
}

fn assert_cli_error(args: &[&str], expected: &str) {
    let output = run_cli(args);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bundle-skills"))
        .args(args)
        .output()
        .unwrap()
}

async fn write_runtime_package(root: &Path, id: &str) {
    tokio::fs::create_dir_all(root).await.unwrap();
    let descriptor = serde_json::json!({
        "schemaVersion": 1,
        "id": id,
        "version": "0.1.0",
        "displayName": "Echo",
        "kind": "native_runtime",
        "package": { "includeInstructions": false, "includeRuntime": true },
        "compatibility": { "platforms": ["desktop", "server"] },
        "requires": {
            "packages": [],
            "capabilities": ["shell.process"],
            "runtimeTools": [],
            "connectors": []
        }
    });
    tokio::fs::write(
        root.join("general-agent.json"),
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    let runtime = serde_json::json!({
        "name": "echo",
        "description": "Echo a text payload.",
        "version": "0.1.0",
        "entry": { "type": "command", "command": "node", "args": ["index.js"] },
        "tools": [{
            "name": "echo",
            "description": "Echo input.",
            "input_schema": { "type": "object" }
        }]
    });
    tokio::fs::write(
        root.join("skill.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.join("index.js"),
        "let data='';process.stdin.on('data',c=>data+=c);process.stdin.on('end',()=>process.stdout.write(data));\n",
    )
    .await
    .unwrap();
}

fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("general-agent-{name}-{}", uuid::Uuid::new_v4()))
}

async fn active_generation(output: &Path) -> PathBuf {
    let current: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(output.join(SKILL_BUNDLE_CURRENT_FILE))
            .await
            .unwrap(),
    )
    .unwrap();
    output
        .join(SKILL_BUNDLE_GENERATIONS_DIR)
        .join(current["generation"].as_str().unwrap())
}

async fn remove_test_dir(path: PathBuf) {
    let _ = tokio::fs::remove_dir_all(path).await;
}
