use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Default)]
pub struct StreamingToolCallAssembler {
    calls: BTreeMap<usize, PartialToolCall>,
}

impl StreamingToolCallAssembler {
    pub fn push(&mut self, delta: ToolCallDelta) -> anyhow::Result<Option<CompletedToolCall>> {
        let call = self.calls.entry(delta.index).or_default();
        if let Some(call_id) = delta.call_id {
            call.call_id = Some(call_id);
        }
        if let Some(name) = delta.name {
            call.name = Some(name);
        }
        if let Some(arguments_delta) = delta.arguments_delta {
            call.arguments.push_str(&arguments_delta);
        }

        Ok(call.try_completed())
    }

    pub fn finish_all(&self) -> anyhow::Result<Vec<CompletedToolCall>> {
        self.calls
            .values()
            .map(PartialToolCall::completed)
            .collect()
    }
}

#[derive(Debug, Default)]
struct PartialToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn try_completed(&self) -> Option<CompletedToolCall> {
        self.completed().ok()
    }

    fn completed(&self) -> anyhow::Result<CompletedToolCall> {
        let call_id = self
            .call_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("streaming tool call missing call id"))?;
        let name = self
            .name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("streaming tool call missing name"))?;
        let arguments = if self.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&self.arguments)?
        };

        Ok(CompletedToolCall {
            call_id,
            name,
            arguments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_streaming_tool_arguments_before_emitting_call() {
        let mut assembler = StreamingToolCallAssembler::default();

        assert!(
            assembler
                .push(ToolCallDelta {
                    index: 0,
                    call_id: Some("call-1".into()),
                    name: Some("create_directory".into()),
                    arguments_delta: Some("{\"path\"".into()),
                })
                .unwrap()
                .is_none()
        );
        let completed = assembler
            .push(ToolCallDelta {
                index: 0,
                call_id: None,
                name: None,
                arguments_delta: Some(":\"test\"}".into()),
            })
            .unwrap()
            .unwrap();

        assert_eq!(completed.call_id, "call-1");
        assert_eq!(completed.name, "create_directory");
        assert_eq!(completed.arguments["path"], "test");
    }

    #[test]
    fn assembler_keeps_parallel_tool_indices_separate() {
        let mut assembler = StreamingToolCallAssembler::default();

        assembler
            .push(ToolCallDelta {
                index: 0,
                call_id: Some("call-a".into()),
                name: Some("read_text_file".into()),
                arguments_delta: Some("{\"path\":\"a.txt\"}".into()),
            })
            .unwrap();
        assembler
            .push(ToolCallDelta {
                index: 1,
                call_id: Some("call-b".into()),
                name: Some("read_text_file".into()),
                arguments_delta: Some("{\"path\":\"b.txt\"}".into()),
            })
            .unwrap();

        let calls = assembler.finish_all().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["path"], "a.txt");
        assert_eq!(calls[1].arguments["path"], "b.txt");
    }
}
