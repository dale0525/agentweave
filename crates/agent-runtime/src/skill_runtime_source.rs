use crate::skill::{SkillRegistry, SkillTool};

impl SkillRegistry {
    pub(crate) fn tools_with_runtime_sources(
        &self,
    ) -> Vec<(String, String, Option<String>, SkillTool)> {
        self.skills
            .iter()
            .filter(|skill| self.skill_is_available(skill))
            .flat_map(|skill| {
                let binding = skill
                    .verification
                    .as_ref()
                    .and_then(|verification| verification.execution_binding.as_ref());
                let package_id = binding.map_or_else(
                    || skill.manifest.name.clone(),
                    |binding| binding.package_id.as_str().to_string(),
                );
                let revision_id = binding.map(|binding| binding.revision_id.clone());
                skill.manifest.tools.clone().into_iter().map(move |tool| {
                    (
                        skill.manifest.name.clone(),
                        package_id.clone(),
                        revision_id.clone(),
                        tool,
                    )
                })
            })
            .collect()
    }
}
