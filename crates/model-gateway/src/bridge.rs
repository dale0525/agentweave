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
                    let content = item.get("content")?.as_str()?;

                    Some(json!({
                        "role": role,
                        "content": content,
                    }))
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

    Ok(chat)
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
}
