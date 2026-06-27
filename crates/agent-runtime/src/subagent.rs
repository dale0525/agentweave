use crate::events::RuntimeEvent;

#[derive(Debug, Default, Clone)]
pub struct SubagentService;

impl SubagentService {
    pub async fn run_fake_task(&self, task: impl Into<String>) -> Vec<RuntimeEvent> {
        let subagent_id = "subagent-1".to_string();
        vec![
            RuntimeEvent::SubagentStarted {
                subagent_id: subagent_id.clone(),
                task: task.into(),
            },
            RuntimeEvent::SubagentFinished { subagent_id },
        ]
    }

    pub async fn fail_fake_task(
        &self,
        task: impl Into<String>,
        message: impl Into<String>,
    ) -> Vec<RuntimeEvent> {
        let subagent_id = "subagent-1".to_string();
        vec![
            RuntimeEvent::SubagentStarted {
                subagent_id: subagent_id.clone(),
                task: task.into(),
            },
            RuntimeEvent::SubagentFailed {
                subagent_id,
                message: message.into(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RuntimeEvent;

    #[tokio::test]
    async fn subagent_service_emits_started_and_finished_events() {
        let service = SubagentService;

        let events = service.run_fake_task("review phase 7").await;

        assert!(matches!(events[0], RuntimeEvent::SubagentStarted { .. }));
        assert!(matches!(events[1], RuntimeEvent::SubagentFinished { .. }));
    }

    #[tokio::test]
    async fn subagent_service_emits_failed_on_error() {
        let service = SubagentService;

        let events = service.fail_fake_task("review phase 7", "timeout").await;

        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::SubagentFailed { message, .. } if message == "timeout"
        )));
    }
}
