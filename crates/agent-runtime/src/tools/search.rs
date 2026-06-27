use super::{
    RuntimeConfig, ToolDefinition, ToolPermission,
    result::{ToolError, ToolResult, ToolResultMetadata},
};
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

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        SEARCH_FILES,
        call_id,
        ToolError {
            code: "not_implemented".to_string(),
            message: "search_files is registered but not implemented yet".to_string(),
            retryable: false,
        },
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..ToolResultMetadata::default()
        },
    )
}
