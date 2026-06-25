use crate::events::RuntimeEvent;
use crate::skill::SkillRegistry;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use model_gateway::responses::GatewayEvent;
use serde_json::Value;
use std::pin::Pin;
use uuid::Uuid;

pub type ModelEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<GatewayEvent>> + Send>>;

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn stream(&self, input: Vec<Value>) -> anyhow::Result<ModelEventStream>;
}

pub struct TurnRunner<C> {
    model: C,
    skills: SkillRegistry,
    max_steps: usize,
}

impl<C> TurnRunner<C>
where
    C: ModelClient,
{
    pub fn new(model: C, skills: SkillRegistry) -> Self {
        Self {
            model,
            skills,
            max_steps: 8,
        }
    }

    pub async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        let turn_id = Uuid::new_v4().to_string();
        let mut events = vec![RuntimeEvent::TurnStarted {
            turn_id: turn_id.clone(),
        }];
        let mut input = vec![serde_json::json!({ "role": "user", "content": user_text })];
        let mut final_text = String::new();

        for _step in 0..self.max_steps {
            let mut stream = self.model.stream(input.clone()).await?;
            let mut saw_tool = false;

            while let Some(event) = stream.next().await {
                match event? {
                    GatewayEvent::TextDelta { text } => {
                        final_text.push_str(&text);
                        events.push(RuntimeEvent::AssistantTextDelta { text });
                    }
                    GatewayEvent::ReasoningDelta { text } => {
                        events.push(RuntimeEvent::ReasoningDelta { text });
                    }
                    GatewayEvent::ToolCall {
                        call_id,
                        name,
                        arguments,
                    } => {
                        saw_tool = true;
                        events.push(RuntimeEvent::ToolCallStarted {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        });
                        let result = self.skills.execute(&name, arguments).await?;
                        events.push(RuntimeEvent::ToolCallFinished {
                            call_id: call_id.clone(),
                            result: result.clone(),
                        });
                        input.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result
                        }));
                    }
                    GatewayEvent::Completed => {}
                    GatewayEvent::Error { message } => {
                        events.push(RuntimeEvent::TurnFailed {
                            turn_id: turn_id.clone(),
                            message,
                        });
                        return Ok(events);
                    }
                    GatewayEvent::ResponseStarted { .. } | GatewayEvent::Usage { .. } => {}
                }
            }

            if !saw_tool {
                events.push(RuntimeEvent::AssistantMessageFinished { text: final_text });
                events.push(RuntimeEvent::TurnFinished { turn_id });
                return Ok(events);
            }
        }

        events.push(RuntimeEvent::TurnFailed {
            turn_id,
            message: "max agent steps exceeded".into(),
        });
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelClient for FakeModel {
        async fn stream(&self, _input: Vec<Value>) -> anyhow::Result<ModelEventStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call == 0 {
                vec![
                    Ok(GatewayEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: "echo".into(),
                        arguments: serde_json::json!({ "text": "hello" }),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            } else {
                vec![
                    Ok(GatewayEvent::TextDelta {
                        text: "done".into(),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn executes_tool_and_continues_until_text_response() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let skills = SkillRegistry::load(skills_root).await.unwrap();
        let runner = TurnRunner::new(
            FakeModel {
                calls: AtomicUsize::new(0),
            },
            skills,
        );

        let events = runner.run("echo hello").await.unwrap();

        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::TurnFinished { .. })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolCallFinished { call_id, .. } if call_id == "call-1"
        )));
    }
}
