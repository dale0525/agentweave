use crate::provider::{EndpointType, ProviderProfile};
use crate::responses::GatewayTool;
use crate::responses::{
    GatewayEvent, GatewayRequest, gateway_request_body_with_tool_map,
    parse_gateway_response_with_tool_map,
};
use crate::tool_identity::ToolNameMap;
use std::collections::BTreeMap;

#[test]
fn tool_identity_wire_names_are_provider_safe_bounded_and_reversible() {
    let tools = vec![
        GatewayTool::new(
            "com.example.calendar/create_event",
            "Create event",
            serde_json::json!({ "type": "object" }),
        ),
        GatewayTool::new(
            "com.example.tasks/create_event",
            "Create task event",
            serde_json::json!({ "type": "object" }),
        ),
        GatewayTool::new(
            format!("com.example.long/{}", "very_long_local_name_".repeat(8)),
            "Exercise provider length bounds",
            serde_json::json!({ "type": "object" }),
        ),
    ];

    let first_map = ToolNameMap::from_tools(&tools).unwrap();
    let second_map = ToolNameMap::from_tools(&tools).unwrap();
    let first = first_map
        .wire_name("com.example.calendar/create_event")
        .unwrap();
    let second = first_map
        .wire_name("com.example.tasks/create_event")
        .unwrap();

    assert_ne!(first, second);
    assert_eq!(
        first,
        second_map
            .wire_name("com.example.calendar/create_event")
            .unwrap()
    );
    assert!(first.len() <= 64);
    assert!(
        first
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    );
    assert_eq!(
        first_map.canonical_name(first).unwrap(),
        "com.example.calendar/create_event"
    );
    for tool in &tools {
        let wire = first_map.wire_name(&tool.id).unwrap();
        assert!(wire.len() <= 64);
        assert!(
            wire.chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        );
    }
}

#[test]
fn tool_identity_rejects_duplicate_canonical_ids_and_wire_collisions() {
    let duplicate = vec![
        tool("com.example.calendar/create_event"),
        tool("com.example.calendar/create_event"),
    ];
    assert_eq!(
        ToolNameMap::from_tools(&duplicate).unwrap_err().to_string(),
        "duplicate canonical tool id"
    );

    let colliding = vec![
        tool("com.example.calendar/create_event"),
        tool("com.example.tasks/create_event"),
    ];
    assert_eq!(
        ToolNameMap::from_tools_with_test_encoder(&colliding, |_| "same_wire".into())
            .unwrap_err()
            .to_string(),
        "provider tool name collision"
    );
}

#[test]
fn tool_identity_maps_definitions_and_assistant_history_for_both_endpoints() {
    let canonical = "com.example.calendar/create_event";
    for endpoint_type in [EndpointType::ChatCompletions, EndpointType::Responses] {
        let request = request_with_history(canonical);
        let map = ToolNameMap::from_tools(&request.tools).unwrap();
        let wire = map.wire_name(canonical).unwrap();
        let body =
            gateway_request_body_with_tool_map(&profile(endpoint_type), request, &map).unwrap();

        match endpoint_type {
            EndpointType::ChatCompletions => {
                assert_eq!(body["tools"][0]["function"]["name"], wire);
                assert_eq!(
                    body["messages"][1]["tool_calls"][0]["function"]["name"],
                    wire
                );
            }
            EndpointType::Responses => {
                assert_eq!(body["tools"][0]["name"], wire);
                assert_eq!(body["input"][1]["name"], wire);
            }
            EndpointType::Completion => unreachable!(),
        }
        assert!(!body.to_string().contains(canonical));
    }
}

#[test]
fn tool_identity_reverse_maps_calls_for_both_endpoints_and_rejects_unknown_wire_names() {
    let canonical = "com.example.calendar/create_event";
    let tools = vec![tool(canonical)];
    let map = ToolNameMap::from_tools(&tools).unwrap();
    let wire = map.wire_name(canonical).unwrap();

    let cases = [
        (
            EndpointType::ChatCompletions,
            serde_json::json!({
                "choices": [{ "message": { "tool_calls": [{
                    "id": "call-chat",
                    "function": { "name": wire, "arguments": "{}" }
                }] } }]
            }),
        ),
        (
            EndpointType::Responses,
            serde_json::json!({
                "output": [{
                    "type": "function_call",
                    "call_id": "call-responses",
                    "name": wire,
                    "arguments": "{}"
                }]
            }),
        ),
    ];

    for (endpoint_type, response) in cases {
        let events =
            parse_gateway_response_with_tool_map(&profile(endpoint_type), response, &map).unwrap();
        assert!(matches!(
            &events[0],
            GatewayEvent::ToolCall { name, .. } if name == canonical
        ));

        let unknown = match endpoint_type {
            EndpointType::ChatCompletions => serde_json::json!({
                "choices": [{ "message": { "tool_calls": [{
                    "id": "call-unknown",
                    "function": { "name": "unknown_wire", "arguments": "{}" }
                }] } }]
            }),
            EndpointType::Responses => serde_json::json!({
                "output": [{
                    "type": "function_call",
                    "call_id": "call-unknown",
                    "name": "unknown_wire",
                    "arguments": "{}"
                }]
            }),
            EndpointType::Completion => unreachable!(),
        };
        assert_eq!(
            parse_gateway_response_with_tool_map(&profile(endpoint_type), unknown, &map,)
                .unwrap_err()
                .to_string(),
            "unknown provider tool name"
        );
    }
}

#[tokio::test]
async fn tool_identity_maps_do_not_leak_between_concurrent_providers_or_retries() {
    let first_canonical = "com.example.calendar/create_event";
    let second_canonical = "com.example.tasks/create_event";
    let first_map = ToolNameMap::from_tools_with_test_encoder(&[tool(first_canonical)], |_| {
        "shared_wire".into()
    })
    .unwrap();
    let second_map = ToolNameMap::from_tools_with_test_encoder(&[tool(second_canonical)], |_| {
        "shared_wire".into()
    })
    .unwrap();

    let parse_chat = |map: ToolNameMap| async move {
        parse_gateway_response_with_tool_map(
            &profile(EndpointType::ChatCompletions),
            serde_json::json!({
                "choices": [{ "message": { "tool_calls": [{
                    "id": "call-1",
                    "function": { "name": "shared_wire", "arguments": "{}" }
                }] } }]
            }),
            &map,
        )
        .unwrap()
    };
    let parse_responses = |map: ToolNameMap| async move {
        parse_gateway_response_with_tool_map(
            &profile(EndpointType::Responses),
            serde_json::json!({
                "output": [{
                    "type": "function_call",
                    "call_id": "call-2",
                    "name": "shared_wire",
                    "arguments": "{}"
                }]
            }),
            &map,
        )
        .unwrap()
    };
    let retry_map = first_map.clone();
    let (first, second, retry) = tokio::join!(
        parse_chat(first_map),
        parse_responses(second_map),
        parse_chat(retry_map),
    );

    assert!(matches!(
        &first[0],
        GatewayEvent::ToolCall { name, .. } if name == first_canonical
    ));
    assert!(matches!(
        &second[0],
        GatewayEvent::ToolCall { name, .. } if name == second_canonical
    ));
    assert!(matches!(
        &retry[0],
        GatewayEvent::ToolCall { name, .. } if name == first_canonical
    ));
}

fn tool(id: &str) -> GatewayTool {
    GatewayTool::new(id, "Test tool", serde_json::json!({ "type": "object" }))
}

fn request_with_history(canonical: &str) -> GatewayRequest {
    GatewayRequest {
        input: vec![
            serde_json::json!({ "role": "user", "content": "create it" }),
            serde_json::json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": canonical, "arguments": "{}" }
                }]
            }),
        ],
        tools: vec![tool(canonical)],
    }
}

fn profile(endpoint_type: EndpointType) -> ProviderProfile {
    ProviderProfile {
        id: "test".into(),
        name: "Test".into(),
        endpoint_type,
        base_url: "https://example.invalid/v1".into(),
        model: "test-model".into(),
        api_key: None,
        headers: BTreeMap::new(),
    }
}
