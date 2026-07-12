use crate::provider::{EndpointType, ProviderProfile};
use crate::responses::{GatewayHttpClient, GatewayRequest, GatewayTool, gateway_request_body};
use crate::tool_identity::ToolNameMap;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[test]
fn gateway_tool_serializes_legacy_name_and_round_trips_name_or_id_input() {
    let legacy = json!({
        "name": "com.example.calendar/create_event",
        "description": "Create an event.",
        "input_schema": { "type": "object" }
    });
    let tool: GatewayTool = serde_json::from_value(legacy.clone()).unwrap();

    assert_eq!(tool.id, "com.example.calendar/create_event");
    assert_eq!(serde_json::to_value(&tool).unwrap(), legacy);
    assert_eq!(
        serde_json::from_value::<GatewayTool>(serde_json::to_value(&tool).unwrap()).unwrap(),
        tool
    );

    let id_input = json!({
        "id": "com.example.tasks/create_task",
        "description": "Create a task.",
        "input_schema": { "type": "object" }
    });
    let tool_from_id: GatewayTool = serde_json::from_value(id_input).unwrap();
    let serialized = serde_json::to_value(tool_from_id).unwrap();
    assert_eq!(serialized["name"], "com.example.tasks/create_task");
    assert_eq!(serialized["id"], Value::Null);
}

#[test]
fn gateway_request_round_trip_preserves_canonical_and_alias_tool_wires() {
    let canonical = "com.example.calendar/create_event";
    let request = GatewayRequest {
        input: vec![json!({ "role": "user", "content": "create it" })],
        tools: vec![
            GatewayTool::new(canonical, "Create an event.", json!({ "type": "object" })),
            GatewayTool::advertised_alias(
                canonical,
                "create_event",
                "Legacy create event alias.",
                json!({ "type": "object" }),
            ),
        ],
    };

    let serialized = serde_json::to_value(&request).unwrap();
    assert_eq!(serialized["tools"][0]["name"], canonical);
    assert_eq!(serialized["tools"][0]["id"], Value::Null);
    assert_eq!(serialized["tools"][1]["name"], "create_event");
    assert_eq!(serialized["tools"][1]["id"], canonical);

    let round_tripped: GatewayRequest = serde_json::from_value(serialized).unwrap();
    assert_eq!(round_tripped, request);
    let map = ToolNameMap::from_tools(&round_tripped.tools).unwrap();
    let canonical_wire = map.wire_name_for_tool(&round_tripped.tools[0]).unwrap();
    let alias_wire = map.wire_name_for_tool(&round_tripped.tools[1]).unwrap();

    assert_ne!(canonical_wire, alias_wire);
    assert_eq!(map.canonical_name(canonical_wire), Some(canonical));
    assert_eq!(map.canonical_name(alias_wire), Some(canonical));
}

#[test]
fn gateway_tool_alias_only_round_trip_does_not_become_canonical() {
    let alias = GatewayTool::advertised_alias(
        "com.example.calendar/create_event",
        "create_event",
        "Legacy create event alias.",
        json!({ "type": "object" }),
    );

    let round_tripped: GatewayTool =
        serde_json::from_value(serde_json::to_value(&alias).unwrap()).unwrap();

    assert_eq!(round_tripped, alias);
    assert_eq!(round_tripped.id, "com.example.calendar/create_event");
    assert_eq!(round_tripped.advertised_name(), "create_event");
}

#[test]
fn completion_rejects_nonempty_tools_before_duplicate_id_validation() {
    let request = GatewayRequest {
        input: vec![json!({ "role": "user", "content": "plain prompt" })],
        tools: vec![tool("duplicate/tool"), tool("duplicate/tool")],
    };

    let error = gateway_request_body(&completion_profile(), request).unwrap_err();

    assert_eq!(error.to_string(), "model_endpoint_does_not_support_tools");
}

#[tokio::test]
async fn completion_stream_rejects_tools_before_duplicate_id_validation() {
    let client = GatewayHttpClient::new(completion_profile());
    let request = GatewayRequest {
        input: vec![json!({ "role": "user", "content": "plain prompt" })],
        tools: vec![tool("duplicate/tool"), tool("duplicate/tool")],
    };

    let error = match client.stream(request).await {
        Ok(_) => panic!("Completion stream unexpectedly accepted tools"),
        Err(error) => error,
    };

    assert_eq!(error.to_string(), "model_endpoint_does_not_support_tools");
}

#[test]
fn completion_without_tools_ignores_prior_tool_history_and_uses_latest_text() {
    let request = GatewayRequest {
        input: vec![
            json!({ "role": "user", "content": "old prompt" }),
            json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "call-old",
                    "type": "function",
                    "function": {
                        "name": "unavailable/tool",
                        "arguments": "{}"
                    }
                }]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "call-old",
                "content": "old result"
            }),
            json!({ "role": "user", "content": "latest prompt" }),
        ],
        tools: Vec::new(),
    };

    let body = gateway_request_body(&completion_profile(), request).unwrap();

    assert_eq!(body["prompt"], "latest prompt");
    assert_eq!(body["tools"], Value::Null);
}

fn tool(id: &str) -> GatewayTool {
    GatewayTool::new(id, "Test tool", json!({ "type": "object" }))
}

fn completion_profile() -> ProviderProfile {
    ProviderProfile {
        id: "legacy".into(),
        name: "Legacy".into(),
        endpoint_type: EndpointType::Completion,
        base_url: "https://example.invalid/v1".into(),
        model: "legacy-model".into(),
        api_key: None,
        headers: BTreeMap::new(),
    }
}
