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
        errors.push("tool name must be a model-safe local or canonical identity".to_string());
    }
    if let Some(namespace) = &definition.namespace
        && !is_valid_namespace(namespace)
    {
        errors
            .push("tool namespace must be a model-safe package or connector identity".to_string());
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
    if let Some((package, local)) = value.split_once('/') {
        return !local.contains('/')
            && crate::skill_package::SkillPackageId::parse(package).is_ok()
            && is_valid_local_name(local);
    }
    is_valid_local_name(value)
}

fn is_valid_local_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn is_valid_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}
