use crate::skill::{InstalledSkill, SkillRegistry, SkillTool};
use crate::tools::ToolSource;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeToolBinding {
    pub(crate) skill_index: usize,
    pub(crate) canonical_id: String,
    pub(crate) local_name: String,
    pub(crate) tool: SkillTool,
    pub(crate) source: ToolSource,
}

impl SkillRegistry {
    pub(crate) fn tools_with_runtime_sources(&self) -> Vec<RuntimeToolBinding> {
        self.runtime_tools(false)
    }

    fn runtime_tools(&self, include_unavailable: bool) -> Vec<RuntimeToolBinding> {
        self.skills
            .iter()
            .enumerate()
            .filter(|(_, skill)| include_unavailable || self.skill_is_available(skill))
            .flat_map(|(skill_index, skill)| {
                let identity = runtime_identity(skill);
                skill.manifest.tools.clone().into_iter().map(move |tool| {
                    let local_name = tool.name.clone();
                    RuntimeToolBinding {
                        skill_index,
                        canonical_id: format!("{}/{local_name}", identity.package_id),
                        local_name,
                        tool,
                        source: ToolSource::RuntimeSkill {
                            skill_name: skill.manifest.name.clone(),
                            package_id: identity.package_id.clone(),
                            revision_id: identity.revision_id.clone(),
                        },
                    }
                })
            })
            .collect()
    }

    pub(crate) fn resolve_runtime_tool(&self, name: &str) -> Option<RuntimeToolBinding> {
        resolve_runtime_tool(self.tools_with_runtime_sources(), name)
    }

    pub(crate) fn resolve_runtime_tool_for_execution(
        &self,
        name: &str,
    ) -> Option<RuntimeToolBinding> {
        resolve_runtime_tool(self.runtime_tools(true), name)
    }
}

fn resolve_runtime_tool(tools: Vec<RuntimeToolBinding>, name: &str) -> Option<RuntimeToolBinding> {
    if let Some(binding) = tools.iter().find(|tool| tool.canonical_id == name) {
        return Some(binding.clone());
    }

    let mut local_matches = tools.into_iter().filter(|tool| tool.local_name == name);
    let binding = local_matches.next()?;
    local_matches.next().is_none().then_some(binding)
}

pub(crate) fn validate_runtime_identities(skills: &[InstalledSkill]) -> anyhow::Result<()> {
    let mut canonical_ids = HashSet::new();
    for skill in skills {
        let identity = runtime_identity(skill);
        if identity.manifest_fallback {
            validate_development_package_identity(&identity.package_id)?;
        }
        for tool in &skill.manifest.tools {
            let canonical_id = format!("{}/{}", identity.package_id, tool.name);
            if !canonical_ids.insert(canonical_id) {
                anyhow::bail!("duplicate canonical runtime tool id");
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_development_package_identity(identity: &str) -> anyhow::Result<()> {
    let valid = !identity.is_empty()
        && identity.len() <= 128
        && identity.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || character == '.'
                || character == '_'
                || character == '-'
        });
    anyhow::ensure!(valid, "invalid runtime package identity: {identity}");
    Ok(())
}

struct RuntimeIdentity {
    package_id: String,
    revision_id: Option<String>,
    manifest_fallback: bool,
}

fn runtime_identity(skill: &InstalledSkill) -> RuntimeIdentity {
    let binding = skill
        .verification
        .as_ref()
        .and_then(|verification| verification.execution_binding.as_ref());
    if let Some(binding) = binding {
        return RuntimeIdentity {
            package_id: binding.package_id.as_str().to_string(),
            revision_id: Some(binding.revision_id.clone()),
            manifest_fallback: false,
        };
    }

    RuntimeIdentity {
        package_id: skill
            .development_package_id
            .clone()
            .unwrap_or_else(|| skill.manifest.name.clone()),
        revision_id: None,
        manifest_fallback: skill.development_package_id.is_none(),
    }
}
