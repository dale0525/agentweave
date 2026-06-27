use super::{ToolDefinition, ToolPermission, result::ToolResult};
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

pub async fn execute(call_id: &str, _arguments: Value, started: Instant) -> ToolResult {
    ToolResult::success(
        APPLY_PATCH,
        call_id,
        json!({ "changed_files": [] }),
        super::result::ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..super::result::ToolResultMetadata::default()
        },
    )
}
