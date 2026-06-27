use crate::{
    bridge::responses_to_chat_completions,
    provider::{EndpointType, ProviderProfile},
};
use futures::{Stream, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::pin::Pin;

pub type GatewayEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<GatewayEvent>> + Send>>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GatewayTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GatewayRequest {
    pub input: Vec<Value>,
    pub tools: Vec<GatewayTool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayEvent {
    ResponseStarted {
        response_id: String,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: Value,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Completed,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct GatewayHttpClient {
    profile: ProviderProfile,
    client: reqwest::Client,
}

impl GatewayHttpClient {
    pub fn new(profile: ProviderProfile) -> Self {
        Self {
            profile,
            client: reqwest::Client::new(),
        }
    }

    pub async fn stream(&self, request: GatewayRequest) -> anyhow::Result<GatewayEventStream> {
        let body = gateway_request_body(&self.profile, request)?;
        let mut builder = self.client.post(self.profile.endpoint_url()).json(&body);

        if let Some(api_key) = &self.profile.api_key
            && !api_key.trim().is_empty()
        {
            builder = builder.bearer_auth(api_key);
        }
        for (name, value) in &self.profile.headers {
            builder = builder.header(name, value);
        }

        let response = builder.send().await?.error_for_status()?.json().await?;
        let events = parse_gateway_response(&self.profile, response)?;

        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

pub fn gateway_request_body(
    profile: &ProviderProfile,
    request: GatewayRequest,
) -> anyhow::Result<Value> {
    if !profile.supports_tools() && !request.tools.is_empty() {
        anyhow::bail!("model_endpoint_does_not_support_tools");
    }

    match profile.endpoint_type {
        EndpointType::Responses => Ok(gateway_responses_body(&profile.model, request)),
        EndpointType::ChatCompletions => {
            responses_to_chat_completions(gateway_base_body(&profile.model, request))
        }
        EndpointType::Completion => Ok(json!({
            "model": profile.model,
            "prompt": latest_text_prompt(&gateway_base_body(&profile.model, request)),
            "stream": false,
        })),
    }
}

fn gateway_base_body(model: &str, request: GatewayRequest) -> Value {
    json!({
        "model": model,
        "input": request.input,
        "tools": request.tools,
        "stream": false,
    })
}

fn gateway_responses_body(model: &str, request: GatewayRequest) -> Value {
    json!({
        "model": model,
        "input": responses_input_items(request.input),
        "tools": responses_tools(request.tools),
        "stream": false,
    })
}

fn responses_tools(tools: Vec<GatewayTool>) -> Vec<Value> {
    tools
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.input_schema,
            })
        })
        .collect()
}

fn responses_input_items(input: Vec<Value>) -> Vec<Value> {
    input.into_iter().flat_map(responses_input_item).collect()
}

fn responses_input_item(item: Value) -> Vec<Value> {
    if item.get("role").and_then(Value::as_str) == Some("assistant")
        && let Some(tool_calls) = item.get("tool_calls").and_then(Value::as_array)
    {
        return tool_calls
            .iter()
            .filter_map(chat_tool_call_to_responses_function_call)
            .collect();
    }

    if item.get("role").and_then(Value::as_str) == Some("tool") {
        return vec![json!({
            "type": "function_call_output",
            "call_id": item.get("tool_call_id").cloned().unwrap_or(Value::Null),
            "output": response_tool_output(item.get("content")),
        })];
    }

    vec![item]
}

fn chat_tool_call_to_responses_function_call(tool_call: &Value) -> Option<Value> {
    let function = tool_call.get("function")?;
    Some(json!({
        "type": "function_call",
        "call_id": tool_call.get("id").cloned().unwrap_or(Value::Null),
        "name": function.get("name").cloned().unwrap_or(Value::Null),
        "arguments": function
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::String("{}".into())),
        "status": "completed",
    }))
}

fn response_tool_output(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

pub fn parse_gateway_response(
    profile: &ProviderProfile,
    response: Value,
) -> anyhow::Result<Vec<GatewayEvent>> {
    match profile.endpoint_type {
        EndpointType::Responses => parse_responses_response(response),
        EndpointType::ChatCompletions => parse_chat_completion_response(response),
        EndpointType::Completion => parse_completion_response(response),
    }
}

fn latest_text_prompt(body: &Value) -> String {
    body.get("input")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().rev().find_map(|item| item.get("content")))
        .map(|content| match content {
            Value::String(text) => text.clone(),
            value => value.to_string(),
        })
        .unwrap_or_default()
}

fn parse_chat_completion_response(response: Value) -> anyhow::Result<Vec<GatewayEvent>> {
    let mut events = Vec::new();
    let message = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| anyhow::anyhow!("chat completion response missing message"))?;

    if let Some(content) = message.get("content").and_then(Value::as_str)
        && !content.is_empty()
    {
        events.push(GatewayEvent::TextDelta {
            text: content.to_string(),
        });
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            if let Some(event) = parse_chat_tool_call(tool_call)? {
                events.push(event);
            }
        }
    }

    events.push(GatewayEvent::Completed);
    Ok(events)
}

fn parse_completion_response(response: Value) -> anyhow::Result<Vec<GatewayEvent>> {
    let text = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("completion response missing text"))?;

    Ok(vec![
        GatewayEvent::TextDelta {
            text: text.to_string(),
        },
        GatewayEvent::Completed,
    ])
}

fn parse_chat_tool_call(tool_call: &Value) -> anyhow::Result<Option<GatewayEvent>> {
    let function = match tool_call.get("function") {
        Some(function) => function,
        None => return Ok(None),
    };
    let name = match function.get("name").and_then(Value::as_str) {
        Some(name) => name,
        None => return Ok(None),
    };
    let call_id = tool_call
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(name)
        .to_string();
    let arguments = parse_arguments(function.get("arguments"))?;

    Ok(Some(GatewayEvent::ToolCall {
        call_id,
        name: name.to_string(),
        arguments,
    }))
}

fn parse_responses_response(response: Value) -> anyhow::Result<Vec<GatewayEvent>> {
    let mut events = Vec::new();

    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for item in output {
            collect_responses_output_item(item, &mut events)?;
        }
    }

    if !events
        .iter()
        .any(|event| matches!(event, GatewayEvent::TextDelta { .. }))
        && let Some(output_text) = response.get("output_text").and_then(Value::as_str)
        && !output_text.is_empty()
    {
        events.push(GatewayEvent::TextDelta {
            text: output_text.to_string(),
        });
    }

    events.push(GatewayEvent::Completed);
    Ok(events)
}

fn collect_responses_output_item(
    item: &Value,
    events: &mut Vec<GatewayEvent>,
) -> anyhow::Result<()> {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("responses function_call missing name"))?;
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or(name);
            events.push(GatewayEvent::ToolCall {
                call_id: call_id.to_string(),
                name: name.to_string(),
                arguments: parse_arguments(item.get("arguments"))?,
            });
        }
        Some("message") => {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    if let Some(text) = part
                        .get("text")
                        .or_else(|| part.get("output_text"))
                        .and_then(Value::as_str)
                    {
                        events.push(GatewayEvent::TextDelta {
                            text: text.to_string(),
                        });
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

fn parse_arguments(arguments: Option<&Value>) -> anyhow::Result<Value> {
    match arguments {
        Some(Value::String(text)) if text.trim().is_empty() => Ok(json!({})),
        Some(Value::String(text)) => Ok(serde_json::from_str(text)?),
        Some(value) => Ok(value.clone()),
        None => Ok(json!({})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{EndpointType, ProviderProfile};
    use std::collections::BTreeMap;

    #[test]
    fn chat_completion_tool_calls_parse_to_gateway_events() {
        let response = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "tool_calls": [
                            {
                                "id": "call-1",
                                "type": "function",
                                "function": {
                                    "name": "echo",
                                    "arguments": "{\"text\":\"hello\"}"
                                }
                            }
                        ]
                    }
                }
            ]
        });

        let events = parse_chat_completion_response(response).unwrap();

        assert_eq!(
            events,
            vec![
                GatewayEvent::ToolCall {
                    call_id: "call-1".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({ "text": "hello" }),
                },
                GatewayEvent::Completed,
            ]
        );
    }

    #[test]
    fn gateway_request_body_includes_runtime_tools_for_chat_completions() {
        let profile = ProviderProfile {
            id: "local".into(),
            name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "http://localhost:11434/v1".into(),
            model: "agent-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![serde_json::json!({ "role": "user", "content": "echo hello" })],
            tools: vec![GatewayTool {
                name: "echo".into(),
                description: "Return text.".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }],
        };

        let body = gateway_request_body(&profile, request).unwrap();

        assert_eq!(body["model"], "agent-model");
        assert_eq!(body["tools"][0]["function"]["name"], "echo");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn chat_completion_body_preserves_tool_result_messages() {
        let profile = ProviderProfile {
            id: "local".into(),
            name: "Local".into(),
            endpoint_type: EndpointType::ChatCompletions,
            base_url: "http://localhost:11434/v1".into(),
            model: "agent-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![
                serde_json::json!({ "role": "user", "content": "echo hello" }),
                serde_json::json!({
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "echo",
                                "arguments": "{\"text\":\"hello\"}"
                            }
                        }
                    ]
                }),
                serde_json::json!({
                    "role": "tool",
                    "tool_call_id": "call-1",
                    "content": { "text": "hello" }
                }),
            ],
            tools: Vec::new(),
        };

        let body = gateway_request_body(&profile, request).unwrap();

        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call-1");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call-1");
        assert_eq!(body["messages"][2]["content"], "{\"text\":\"hello\"}");
    }

    #[test]
    fn gateway_request_body_includes_function_tools_for_responses() {
        let profile = ProviderProfile {
            id: "openai".into(),
            name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5.4".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![serde_json::json!({ "role": "user", "content": "echo hello" })],
            tools: vec![GatewayTool {
                name: "echo".into(),
                description: "Return text.".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }],
        };

        let body = gateway_request_body(&profile, request).unwrap();

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "echo");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
        assert_eq!(body["tools"][0]["input_schema"], Value::Null);
    }

    #[test]
    fn responses_body_converts_chat_tool_result_items_to_function_call_outputs() {
        let profile = ProviderProfile {
            id: "openai".into(),
            name: "OpenAI".into(),
            endpoint_type: EndpointType::Responses,
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5.4".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![
                serde_json::json!({ "role": "user", "content": "echo hello" }),
                serde_json::json!({
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {
                                "name": "echo",
                                "arguments": "{\"text\":\"hello\"}"
                            }
                        }
                    ]
                }),
                serde_json::json!({
                    "role": "tool",
                    "tool_call_id": "call-1",
                    "content": { "text": "hello" }
                }),
            ],
            tools: Vec::new(),
        };

        let body = gateway_request_body(&profile, request).unwrap();

        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["call_id"], "call-1");
        assert_eq!(body["input"][1]["name"], "echo");
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["call_id"], "call-1");
        assert_eq!(body["input"][2]["output"], "{\"text\":\"hello\"}");
    }

    #[test]
    fn completion_response_text_parses_to_gateway_events() {
        let profile = ProviderProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            endpoint_type: EndpointType::Completion,
            base_url: "http://localhost:11434/v1".into(),
            model: "legacy-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let response = serde_json::json!({
            "choices": [
                { "text": "plain completion" }
            ]
        });

        let events = parse_gateway_response(&profile, response).unwrap();

        assert_eq!(
            events,
            vec![
                GatewayEvent::TextDelta {
                    text: "plain completion".into()
                },
                GatewayEvent::Completed,
            ]
        );
    }

    #[test]
    fn completion_body_rejects_tool_schemas() {
        let profile = ProviderProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            endpoint_type: EndpointType::Completion,
            base_url: "http://localhost:11434/v1".into(),
            model: "legacy-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![serde_json::json!({ "role": "user", "content": "echo hello" })],
            tools: vec![GatewayTool {
                name: "echo".into(),
                description: "Return text.".into(),
                input_schema: serde_json::json!({ "type": "object" }),
            }],
        };

        let error = gateway_request_body(&profile, request).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("model_endpoint_does_not_support_tools")
        );
    }

    #[test]
    fn completion_body_without_tools_still_builds_prompt() {
        let profile = ProviderProfile {
            id: "legacy".into(),
            name: "Legacy".into(),
            endpoint_type: EndpointType::Completion,
            base_url: "http://localhost:11434/v1".into(),
            model: "legacy-model".into(),
            api_key: None,
            headers: BTreeMap::new(),
        };
        let request = GatewayRequest {
            input: vec![serde_json::json!({ "role": "user", "content": "plain prompt" })],
            tools: Vec::new(),
        };

        let body = gateway_request_body(&profile, request).unwrap();

        assert_eq!(body["model"], "legacy-model");
        assert_eq!(body["prompt"], "plain prompt");
        assert_eq!(body["stream"], false);
        assert_eq!(body["tools"], Value::Null);
    }

    #[test]
    fn responses_output_text_does_not_duplicate_message_content() {
        let response = serde_json::json!({
            "output_text": "hello",
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "hello"
                        }
                    ]
                }
            ]
        });

        let events = parse_responses_response(response).unwrap();

        assert_eq!(
            events,
            vec![
                GatewayEvent::TextDelta {
                    text: "hello".into()
                },
                GatewayEvent::Completed,
            ]
        );
    }
}
