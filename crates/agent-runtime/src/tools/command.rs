use super::{
    RuntimeConfig, ToolDefinition, ToolPermission,
    result::{ToolError, ToolResult, ToolResultMetadata},
};
use serde_json::{Value, json};
use std::time::Instant;

pub const EXEC_COMMAND: &str = "exec_command";

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        name: EXEC_COMMAND.to_string(),
        description: "Execute a development command inside the workspace.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string" },
                "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1 }
            },
            "required": ["cmd"],
            "additionalProperties": false
        }),
        permission: ToolPermission::ExecuteCommand,
    }
}

pub async fn execute(
    _config: &RuntimeConfig,
    call_id: &str,
    _arguments: Value,
    started: Instant,
) -> ToolResult {
    ToolResult::failure(
        EXEC_COMMAND,
        call_id,
        ToolError {
            code: "command_disabled".to_string(),
            message: "command execution is not implemented in this runtime build".to_string(),
            retryable: false,
        },
        ToolResultMetadata {
            duration_ms: started.elapsed().as_millis() as u64,
            ..ToolResultMetadata::default()
        },
    )
}
