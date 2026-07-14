use crate::events::RuntimeEvent;
use crate::skill::SkillRegistry;
use crate::tools::{RuntimeConfig, ToolExecutionObserver, ToolSource};
use crate::turn::{ModelClient, ModelEventStream, TurnRunner};
use async_trait::async_trait;
use futures::stream;
use model_gateway::responses::{GatewayEvent, GatewayRequest};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct ToolThenTextModel(AtomicUsize);

#[async_trait]
impl ModelClient for ToolThenTextModel {
    async fn stream(&self, _request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        let events = if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                GatewayEvent::ToolCall {
                    call_id: "call-1".into(),
                    name: "observed_echo".into(),
                    legacy_alias_selected: false,
                    arguments: serde_json::json!({"value": 7}),
                },
                GatewayEvent::Completed,
            ]
        } else {
            vec![
                GatewayEvent::TextDelta {
                    text: "done".into(),
                },
                GatewayEvent::Completed,
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

struct FailingObserver;

#[async_trait]
impl ToolExecutionObserver for FailingObserver {
    async fn finished(&self, _source: &ToolSource, _success: bool) -> anyhow::Result<()> {
        anyhow::bail!("private observer failure details")
    }
}

#[tokio::test]
async fn production_turn_surfaces_sanitized_observer_diagnostic() {
    let root =
        std::env::temp_dir().join(format!("agentweave-turn-observer-{}", uuid::Uuid::new_v4()));
    write_skill(&root).await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let runner = TurnRunner::new_with_config(
        ToolThenTextModel(AtomicUsize::new(0)),
        skills,
        RuntimeConfig::workspace_write(root.clone(), root.clone()),
    )
    .with_execution_observer_for_test(Arc::new(FailingObserver));

    let events = runner.run("run the observed tool").await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolObserverDiagnostic { operation, message }
            if operation == "tool_execution_observer"
                && message == "tool execution observer failed"
    )));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

async fn write_skill(root: &std::path::Path) {
    let skill = root.join("observed");
    tokio::fs::create_dir_all(&skill).await.unwrap();
    tokio::fs::write(
        skill.join("skill.json"),
        serde_json::json!({
            "name": "observed",
            "description": "Observed turn tool.",
            "version": "0.1.0",
            "entry": {"type": "command", "command": "node", "args": ["index.js"]},
            "tools": [{
                "name": "observed_echo",
                "description": "Echo input.",
                "permission": "read_workspace",
                "input_schema": {"type": "object"}
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        skill.join("index.js"),
        "process.stdin.on('data', chunk => process.stdout.write(chunk));\n",
    )
    .await
    .unwrap();
}
