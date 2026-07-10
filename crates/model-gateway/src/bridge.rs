//! First incremental bridge slice inspired by cc-switch's Codex Responses
//! <-> Chat conversion.

use serde_json::{Value, json};

pub fn responses_to_chat_completions(body: Value) -> anyhow::Result<Value> {
    let messages = body
        .get("input")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let role = item.get("role")?.as_str()?;

                    let mut message = json!({
                        "role": chat_message_role(role),
                        "content": normalize_message_content(item.get("content")),
                    });

                    if let Some(tool_call_id) = item.get("tool_call_id") {
                        message["tool_call_id"] = tool_call_id.clone();
                    }

                    if let Some(tool_calls) = item.get("tool_calls") {
                        message["tool_calls"] = tool_calls.clone();
                    }

                    Some(message)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut chat = json!({
        "model": body.get("model").cloned().unwrap_or(Value::Null),
        "messages": messages,
    });

    if let Some(stream) = body.get("stream") {
        chat["stream"] = stream.clone();
    }

    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        chat["tools"] = Value::Array(
            tools
                .iter()
                .filter_map(responses_tool_to_chat_tool)
                .collect::<Vec<_>>(),
        );
    }

    Ok(chat)
}

fn normalize_message_content(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(value) => Value::String(value.to_string()),
        None => Value::String(String::new()),
    }
}

fn chat_message_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        role => role,
    }
}

fn responses_tool_to_chat_tool(tool: &Value) -> Option<Value> {
    let name = tool.get("name")?.as_str()?;
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parameters = tool
        .get("input_schema")
        .or_else(|| tool.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object" }));

    Some(json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_responses_text_input_to_chat_messages() {
        let input = serde_json::json!({
            "model": "test-model",
            "input": [
                { "role": "user", "content": "hello" }
            ]
        });

        let chat = responses_to_chat_completions(input).unwrap();

        assert_eq!(chat["model"], "test-model");
        assert_eq!(chat["messages"][0]["role"], "user");
        assert_eq!(chat["messages"][0]["content"], "hello");
    }

    #[test]
    fn converts_responses_tools_to_chat_function_tools() {
        let input = serde_json::json!({
            "model": "test-model",
            "input": [
                { "role": "user", "content": "echo hello" }
            ],
            "tools": [
                {
                    "name": "echo",
                    "description": "Return the provided text.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" }
                        },
                        "required": ["text"]
                    }
                }
            ]
        });

        let chat = responses_to_chat_completions(input).unwrap();

        assert_eq!(chat["tools"][0]["type"], "function");
        assert_eq!(chat["tools"][0]["function"]["name"], "echo");
        assert_eq!(
            chat["tools"][0]["function"]["parameters"]["properties"]["text"]["type"],
            "string"
        );
    }

    #[test]
    fn converts_developer_messages_to_system_messages() {
        let input = serde_json::json!({
            "model": "test-model",
            "input": [
                { "role": "developer", "content": "Project instruction" },
                { "role": "user", "content": "hello" }
            ]
        });

        let chat = responses_to_chat_completions(input).unwrap();

        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][0]["content"], "Project instruction");
        assert_eq!(chat["messages"][1]["role"], "user");
    }
}
