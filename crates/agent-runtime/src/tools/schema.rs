use super::{ToolDefinition, ToolPermission, ToolSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolSchemaDiagnostic {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDiagnostic {
    pub name: String,
    pub namespace: Option<String>,
    pub description: String,
    pub permission: ToolPermission,
    pub source: ToolSource,
    pub schema: ToolSchemaDiagnostic,
}

pub fn validate_tool_definition(definition: &ToolDefinition) -> ToolSchemaDiagnostic {
    let mut errors = Vec::new();

    if !is_valid_tool_identifier(&definition.name) {
        errors.push("tool name must be 1-64 ASCII alphanumeric, '_' or '-' chars".to_string());
    }
    if let Some(namespace) = &definition.namespace
        && !is_valid_tool_identifier(namespace)
    {
        errors.push("tool namespace must be 1-64 ASCII alphanumeric, '_' or '-' chars".to_string());
    }
    if definition
        .input_schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        != Some("object")
    {
        errors.push("tool input_schema type must be object".to_string());
    }
    if let Some(output_schema) = &definition.output_schema
        && !output_schema.is_object()
    {
        errors.push("tool output_schema must be a JSON object".to_string());
    }

    ToolSchemaDiagnostic {
        valid: errors.is_empty(),
        errors,
    }
}

fn is_valid_tool_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}
