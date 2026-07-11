use crate::skill_management::{
    CreateSkillDraftRequest, OwnerSkillManagementService, SkillManagementError,
};
use crate::skill_package::SkillPackageKind;
use crate::skill_policy::{ActorContext, SkillManagementPolicy, SkillOperation};
use crate::tools::result::{ToolError, ToolResult, ToolResultMetadata};
use crate::tools::{ToolDefinition, ToolPermission, ToolSource};
use serde_json::{Value, json};

pub const CREATE_SKILL_DRAFT_TOOL: &str = "create_skill_draft";

#[derive(Clone)]
pub struct SkillManagementToolContext {
    pub service: OwnerSkillManagementService,
    pub actor: ActorContext,
}

impl std::fmt::Debug for SkillManagementToolContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillManagementToolContext")
            .field("actor", &self.actor)
            .finish_non_exhaustive()
    }
}

pub struct SkillManagementTools;

impl SkillManagementTools {
    pub(crate) fn is_reserved_name(name: &str) -> bool {
        name == CREATE_SKILL_DRAFT_TOOL
    }

    pub fn definitions(
        service: &OwnerSkillManagementService,
        actor: &ActorContext,
    ) -> Vec<ToolDefinition> {
        Self::definitions_for_policy(service.policy(), actor)
    }

    pub fn definitions_for_policy(
        policy: &SkillManagementPolicy,
        actor: &ActorContext,
    ) -> Vec<ToolDefinition> {
        let allowed_kinds = [
            (SkillPackageKind::InstructionOnly, "instruction_only"),
            (SkillPackageKind::HostToolsOnly, "host_tools_only"),
        ]
        .into_iter()
        .filter_map(|(kind, name)| {
            policy
                .allows(actor, SkillOperation::CreateDraft, kind)
                .then_some(name)
        })
        .collect::<Vec<_>>();
        if allowed_kinds.is_empty() {
            return Vec::new();
        }
        vec![ToolDefinition {
            name: CREATE_SKILL_DRAFT_TOOL.into(),
            namespace: Some("generalagent_skill_management".into()),
            description: "Create an inactive owner-managed skill draft.".into(),
            input_schema: create_draft_schema(&allowed_kinds),
            output_schema: None,
            permission: ToolPermission::ManageSkills,
            source: ToolSource::BuiltIn,
        }]
    }

    pub fn handles(context: &SkillManagementToolContext, name: &str) -> bool {
        Self::definitions(&context.service, &context.actor)
            .iter()
            .any(|definition| definition.name == name)
    }

    pub async fn execute(
        context: &SkillManagementToolContext,
        name: &str,
        call_id: &str,
        arguments: Value,
    ) -> ToolResult {
        if name != CREATE_SKILL_DRAFT_TOOL {
            return failure(
                name,
                call_id,
                "unknown_tool",
                format!("unknown tool: {name}"),
            );
        }
        let request = match serde_json::from_value::<CreateSkillDraftRequest>(arguments) {
            Ok(request) => request,
            Err(error) => {
                return failure(name, call_id, "invalid_arguments", error.to_string());
            }
        };
        match context.service.create_draft(&context.actor, request).await {
            Ok(summary) => match serde_json::to_value(summary) {
                Ok(value) => {
                    ToolResult::success(name, call_id, value, ToolResultMetadata::default())
                }
                Err(error) => failure(name, call_id, "internal_error", error.to_string()),
            },
            Err(error) => {
                let (code, message) = classify_error(&error);
                failure(name, call_id, code, message)
            }
        }
    }
}

fn create_draft_schema(allowed_kinds: &[&str]) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["package_id", "display_name", "description", "kind"],
        "properties": {
            "package_id": {"type": "string"},
            "display_name": {"type": "string"},
            "description": {"type": "string"},
            "kind": {
                "type": "string",
                "enum": allowed_kinds
            },
            "required_tools": {
                "type": "array",
                "items": {"type": "string"},
                "default": []
            }
        }
    })
}

fn classify_error(error: &anyhow::Error) -> (&'static str, String) {
    match error.downcast_ref::<SkillManagementError>() {
        Some(SkillManagementError::Denied { .. }) => ("permission_denied", error.to_string()),
        Some(SkillManagementError::InvalidRequest(_)) => ("invalid_arguments", error.to_string()),
        None => ("internal_error", "skill management operation failed".into()),
    }
}

fn failure(tool: &str, call_id: &str, code: &str, message: impl Into<String>) -> ToolResult {
    ToolResult::failure(
        tool,
        call_id,
        ToolError {
            code: code.into(),
            message: message.into(),
            retryable: false,
        },
        ToolResultMetadata::default(),
    )
}
