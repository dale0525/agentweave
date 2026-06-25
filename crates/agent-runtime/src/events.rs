use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    TurnStarted {
        turn_id: String,
    },
    AssistantTextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallStarted {
        call_id: String,
        name: String,
        arguments: Value,
    },
    ToolCallFinished {
        call_id: String,
        result: Value,
    },
    AssistantMessageFinished {
        text: String,
    },
    TurnFinished {
        turn_id: String,
    },
    TurnFailed {
        turn_id: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_event_serializes_with_snake_case_type() {
        let event = RuntimeEvent::AssistantTextDelta {
            text: "hello".into(),
        };

        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["type"], "assistant_text_delta");
        assert_eq!(json["text"], "hello");
    }
}
