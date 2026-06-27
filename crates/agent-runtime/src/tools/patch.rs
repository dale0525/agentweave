use super::{
    RuntimeConfig, ToolDefinition, ToolPermission,
    result::{ToolError, ToolResult, ToolResultMetadata},
};
use serde_json::{Value, json};
use std::time::Instant;

pub const APPLY_PATCH: &str = "apply_patch";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: APPLY_PATCH.to_string(),
        description: "Apply a minimal patch inside the workspace.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string" }
            },
            "required": ["patch"],
            "additionalProperties": false
        }),
        permission: ToolPermission::WriteWorkspace,
    }
}

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        APPLY_PATCH,
        call_id,
        ToolError {
            code: "not_implemented".to_string(),
            message: "apply_patch is registered but not implemented yet".to_string(),
            retryable: false,
        },
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..ToolResultMetadata::default()
        },
    )
}
