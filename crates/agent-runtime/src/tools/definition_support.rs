use super::{
    AgentAppRuntimePolicy, AppNetworkPolicy, ExternalSideEffectPolicy, ExternalToolConfig,
    RuntimeToolBinding, ToolDefinition, ToolDiscoveryItem, ToolPermission, ToolSource, Value,
};
use serde::Serialize;

pub(super) fn runtime_tool_definition(
    binding: &RuntimeToolBinding,
    name: String,
) -> ToolDefinition {
    let namespace = match &binding.source {
        ToolSource::RuntimeSkill { package_id, .. } => Some(package_id.clone()),
        _ => None,
    };
    ToolDefinition {
        name,
        namespace,
        description: binding.tool.description.clone(),
        input_schema: binding.tool.input_schema.clone(),
        output_schema: None,
        permission: binding.tool.permission,
        source: binding.source.clone(),
    }
}

pub(super) fn external_definitions(
    tools: &[ExternalToolConfig],
) -> anyhow::Result<Vec<ToolDefinition>> {
    tools
        .iter()
        .filter_map(|tool| tool.tool_definition().transpose())
        .collect()
}

pub(super) fn external_discovery(
    tools: &[ExternalToolConfig],
) -> anyhow::Result<Vec<ToolDiscoveryItem>> {
    tools
        .iter()
        .map(ExternalToolConfig::discovery_summary)
        .collect()
}

pub(super) fn app_policy_allows_discovery(
    policy: &AgentAppRuntimePolicy,
    item: &ToolDiscoveryItem,
) -> bool {
    app_policy_allows_tool(
        policy,
        &ToolDefinition {
            name: item.name.clone(),
            namespace: item.namespace.clone(),
            description: item.description.clone(),
            input_schema: Value::Null,
            output_schema: None,
            permission: item.permission,
            source: item.source.clone(),
        },
    )
}

pub(super) fn app_policy_allows_tool(
    policy: &AgentAppRuntimePolicy,
    definition: &ToolDefinition,
) -> bool {
    if definition.permission == ToolPermission::ExecuteCommand
        && policy.network() != AppNetworkPolicy::Unrestricted
    {
        return false;
    }
    if !matches!(definition.source, ToolSource::BuiltIn)
        && !policy.declares_runtime_tool(&definition.name)
    {
        return false;
    }
    if policy.external_side_effects() == ExternalSideEffectPolicy::Deny
        && tool_has_external_side_effect(definition)
    {
        return false;
    }
    match &definition.source {
        ToolSource::RuntimeSkill { .. } => policy.network() == AppNetworkPolicy::Unrestricted,
        ToolSource::Mcp { server } => {
            policy.network() != AppNetworkPolicy::Deny && policy.declares_connector(server)
        }
        ToolSource::AppConnector { connector } => {
            policy.network() != AppNetworkPolicy::Deny && policy.declares_connector(connector)
        }
        ToolSource::HostCapability { capability }
            if capability == "agentweave.host.automation/v1" =>
        {
            policy.background_execution()
                != crate::app_manifest::BackgroundExecutionPolicy::Disabled
        }
        ToolSource::HostCapability { capability } => {
            policy.network() == AppNetworkPolicy::Unrestricted
                || matches!(
                    capability.as_str(),
                    "agentweave.host.memory/v1"
                        | "agentweave.host.tasks/v1"
                        | "agentweave.host.attachments/v1"
                )
        }
        ToolSource::BuiltIn => true,
    }
}

pub(super) fn tool_has_external_side_effect(definition: &ToolDefinition) -> bool {
    definition.permission == ToolPermission::ExternalWrite
        || (definition.permission == ToolPermission::DestructiveWrite
            && matches!(
                definition.source,
                ToolSource::RuntimeSkill { .. }
                    | ToolSource::Mcp { .. }
                    | ToolSource::AppConnector { .. }
            ))
}

pub(super) fn serialized_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}
