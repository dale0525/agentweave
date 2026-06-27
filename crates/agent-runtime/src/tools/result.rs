use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub tool: String,
    pub call_id: String,
    pub data: Option<Value>,
    pub error: Option<ToolError>,
    pub metadata: ToolResultMetadata,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolResultMetadata {
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub output_truncated: bool,
}

impl ToolResult {
    pub fn success(
        tool: impl Into<String>,
        call_id: impl Into<String>,
        data: Value,
        metadata: ToolResultMetadata,
    ) -> Self {
        Self {
            ok: true,
            tool: tool.into(),
            call_id: call_id.into(),
            data: Some(data),
            error: None,
            metadata,
        }
    }

    pub fn failure(
        tool: impl Into<String>,
        call_id: impl Into<String>,
        error: ToolError,
        metadata: ToolResultMetadata,
    ) -> Self {
        Self {
            ok: false,
            tool: tool.into(),
            call_id: call_id.into(),
            data: None,
            error: Some(error),
            metadata,
        }
    }

    pub fn into_value(self) -> Value {
        serde_json::to_value(self).expect("tool result should serialize")
    }
}

impl Default for ToolResultMetadata {
    fn default() -> Self {
        Self {
            duration_ms: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            output_truncated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_result_uses_stable_json_envelope() {
        let result = ToolResult::success(
            "read_file",
            "call-1",
            serde_json::json!({ "text": "hello" }),
            ToolResultMetadata {
                duration_ms: 12,
                stdout_truncated: false,
                stderr_truncated: false,
                output_truncated: false,
            },
        );

        assert_eq!(
            result.into_value(),
            serde_json::json!({
                "ok": true,
                "tool": "read_file",
                "call_id": "call-1",
                "data": { "text": "hello" },
                "error": null,
                "metadata": {
                    "duration_ms": 12,
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "output_truncated": false
                }
            })
        );
    }

    #[test]
    fn failure_result_uses_stable_json_envelope() {
        let result = ToolResult::failure(
            "write_file",
            "call-2",
            ToolError {
                code: "permission_denied".into(),
                message: "workspace writes are disabled".into(),
                retryable: false,
            },
            ToolResultMetadata::default(),
        );

        assert_eq!(
            result.into_value(),
            serde_json::json!({
                "ok": false,
                "tool": "write_file",
                "call_id": "call-2",
                "data": null,
                "error": {
                    "code": "permission_denied",
                    "message": "workspace writes are disabled",
                    "retryable": false
                },
                "metadata": {
                    "duration_ms": 0,
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "output_truncated": false
                }
            })
        );
    }
}
