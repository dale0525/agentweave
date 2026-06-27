use super::{ToolDefinition, ToolPermission, result::ToolResult};
use serde_json::{Value, json};
use std::time::Instant;

pub const SEARCH_FILES: &str = "search_files";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: SEARCH_FILES.to_string(),
        description: "Search for text matches inside the workspace.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1 }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        permission: ToolPermission::ReadWorkspace,
    }
}

pub async fn execute(call_id: &str, arguments: Value, started: Instant) -> ToolResult {
    let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
    let pattern = arguments
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or_default();

    ToolResult::success(
        SEARCH_FILES,
        call_id,
        json!({
            "path": path,
            "pattern": pattern,
            "matches": [],
            "truncated": false,
            "engine": "skeleton"
        }),
        super::result::ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..super::result::ToolResultMetadata::default()
        },
    )
}
