use super::{RuntimeConfig, ToolExecutionObserver, ToolRegistry, ToolSource};
use crate::skill::SkillRegistry;
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct GatedObserver {
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl ToolExecutionObserver for GatedObserver {
    async fn finished(&self, _source: &ToolSource, _success: bool) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_waiters();
        self.release.notified().await;
        Ok(())
    }
}

struct FailingObserver;

#[async_trait]
impl ToolExecutionObserver for FailingObserver {
    async fn finished(&self, _source: &ToolSource, _success: bool) -> anyhow::Result<()> {
        anyhow::bail!("observer storage unavailable")
    }
}

struct CountingObserver(AtomicUsize);

#[async_trait]
impl ToolExecutionObserver for CountingObserver {
    async fn finished(&self, _source: &ToolSource, _success: bool) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn observer_failure_does_not_rewrite_the_decided_tool_result() {
    let root = unique_test_dir("observer-result");
    write_skill(&root, "observed", "observed_echo", "read_workspace").await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let registry = ToolRegistry::new(
        skills,
        &RuntimeConfig::workspace_write(root.clone(), root.clone()),
    )
    .with_execution_observer(Arc::new(FailingObserver));

    let result = registry
        .execute("observed_echo", "call-1", serde_json::json!({"value": 7}))
        .await;

    assert!(result.ok);
    assert_eq!(result.data, Some(serde_json::json!({"value": 7})));
    let diagnostics = registry.observer_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].operation, "tool_execution_observer");
    assert!(
        !diagnostics[0]
            .message
            .contains(root.to_string_lossy().as_ref())
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn permission_denial_is_not_reported_as_a_runtime_execution() {
    let root = unique_test_dir("observer-permission");
    write_skill(&root, "blocked", "blocked_write", "write_workspace").await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let observer = Arc::new(CountingObserver(AtomicUsize::new(0)));
    let registry = ToolRegistry::new(
        skills,
        &RuntimeConfig::read_only(root.clone(), root.clone()),
    )
    .with_execution_observer(observer.clone());

    let result = registry
        .execute("blocked_write", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "permission_denied");
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);
    assert!(registry.observer_diagnostics().is_empty());
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_timeout_is_reported_once_as_an_execution_failure() {
    let root = unique_test_dir("observer-timeout");
    write_skill(&root, "slow", "slow_read", "read_workspace").await;
    tokio::fs::write(
        root.join("slow/index.js"),
        "process.stdin.resume(); setTimeout(() => process.stdout.write('{}'), 1000);\n",
    )
    .await
    .unwrap();
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let observer = Arc::new(CountingObserver(AtomicUsize::new(0)));
    let mut config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    config.tool_timeout_ms = 25;
    let registry = ToolRegistry::new(skills, &config).with_execution_observer(observer.clone());

    let result = registry
        .execute("slow_read", "call-1", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "timeout");
    assert_eq!(observer.0.load(Ordering::SeqCst), 1);
    remove_test_dir(root).await;
}

#[tokio::test]
async fn committed_observer_runs_once_outside_the_tool_timeout() {
    let root = unique_test_dir("observer-outside-timeout");
    write_skill(&root, "observed", "observed_echo", "read_workspace").await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let observer = Arc::new(GatedObserver {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let mut config = RuntimeConfig::workspace_write(root.clone(), root.clone());
    config.tool_timeout_ms = 200;
    let registry =
        Arc::new(ToolRegistry::new(skills, &config).with_execution_observer(observer.clone()));
    let worker = registry.clone();
    let execution = tokio::spawn(async move {
        worker
            .execute("observed_echo", "call-1", serde_json::json!({"value": 7}))
            .await
    });

    observer.entered.notified().await;
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    observer.release.notify_waiters();
    let result = execution.await.unwrap();

    assert!(result.ok, "observer latency must not rewrite tool success");
    assert_eq!(observer.calls.load(Ordering::SeqCst), 1);
    remove_test_dir(root).await;
}

async fn write_skill(root: &std::path::Path, folder: &str, tool_name: &str, permission: &str) {
    let skill_dir = root.join(folder);
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(
        skill_dir.join("skill.json"),
        serde_json::json!({
            "name": folder,
            "description": "Observer test runtime skill.",
            "version": "0.1.0",
            "entry": {"type": "command", "command": "node", "args": ["index.js"]},
            "tools": [{
                "name": tool_name,
                "description": "Observer test tool.",
                "permission": permission,
                "input_schema": {"type": "object"}
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        skill_dir.join("index.js"),
        "process.stdin.on('data', chunk => process.stdout.write(chunk));\n",
    )
    .await
    .unwrap();
}

fn unique_test_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
}

async fn remove_test_dir(path: std::path::PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
