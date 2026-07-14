use crate::prompt_composer::{
    AppPromptConfig, DEFAULT_FRAMEWORK_SAFETY_INSTRUCTIONS, PromptAuthority, PromptBlock,
    PromptComposer, PromptComposition,
};
use crate::skill_catalog::{SkillInstructionDocument, SkillSummary};
use anyhow::{Context, bail};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstructionConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub max_instruction_bytes: usize,
    pub max_prompt_bytes: usize,
    pub max_prompt_layer_bytes: usize,
    pub framework_instructions: String,
    pub app_prompt: AppPromptConfig,
    /// Backward-compatible alias for the App system instructions.
    pub base_instructions: String,
    pub developer_instructions: Option<String>,
    pub session_instructions: Option<String>,
    pub memory_context: Option<String>,
    pub goal_instructions: Option<String>,
    pub turn_local_instructions: Option<String>,
    pub skill_summaries: Vec<SkillSummary>,
    pub skill_instructions: Vec<SkillInstructionDocument>,
}

#[derive(Clone, Debug)]
pub struct InstructionDocument {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
    read_bytes: usize,
    original_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct InstructionContext {
    pub config: InstructionConfig,
    documents: Vec<InstructionDocument>,
}

impl InstructionConfig {
    pub fn new(workspace_root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            cwd: cwd.into(),
            max_instruction_bytes: 64 * 1024,
            max_prompt_bytes: 512 * 1024,
            max_prompt_layer_bytes: 64 * 1024,
            framework_instructions: DEFAULT_FRAMEWORK_SAFETY_INSTRUCTIONS.into(),
            app_prompt: AppPromptConfig::default(),
            base_instructions: crate::prompt_composer::DEFAULT_APP_SYSTEM_INSTRUCTIONS.into(),
            developer_instructions: None,
            session_instructions: None,
            memory_context: None,
            goal_instructions: None,
            turn_local_instructions: None,
            skill_summaries: Vec::new(),
            skill_instructions: Vec::new(),
        }
    }
}

impl InstructionContext {
    pub fn load(config: InstructionConfig) -> anyhow::Result<Self> {
        let workspace_root = config.workspace_root.canonicalize().with_context(|| {
            format!(
                "canonicalize workspace root {}",
                config.workspace_root.display()
            )
        })?;
        let cwd = config
            .cwd
            .canonicalize()
            .with_context(|| format!("canonicalize cwd {}", config.cwd.display()))?;

        if !cwd.starts_with(&workspace_root) {
            bail!(
                "cwd {} must be inside workspace root {}",
                cwd.display(),
                workspace_root.display()
            );
        }

        let mut documents = Vec::new();
        for dir in instruction_dirs(&workspace_root, &cwd)? {
            let agents_path = dir.join("AGENTS.md");
            if instruction_file_is_inside_workspace(&workspace_root, &agents_path) {
                documents.push(read_instruction_document(
                    &workspace_root,
                    &agents_path,
                    config.max_instruction_bytes,
                )?);
            }
        }

        Ok(Self {
            config: InstructionConfig {
                workspace_root,
                cwd,
                ..config
            },
            documents,
        })
    }

    pub fn documents(&self) -> &[InstructionDocument] {
        &self.documents
    }

    pub fn model_input(&self, user_text: &str) -> Vec<serde_json::Value> {
        self.try_model_input(user_text, &[])
            .expect("default prompt budgets should compose")
            .input
    }

    pub fn try_model_input(
        &self,
        user_text: &str,
        history: &[serde_json::Value],
    ) -> anyhow::Result<PromptComposition> {
        PromptComposer::new(self.config.max_prompt_bytes).compose(
            self.prompt_blocks(),
            history,
            user_text,
        )
    }

    fn prompt_blocks(&self) -> Vec<PromptBlock> {
        let budget = self.config.max_prompt_layer_bytes;
        let mut blocks = vec![
            PromptBlock::new(
                PromptAuthority::Framework,
                "framework:safety",
                self.config.framework_instructions.clone(),
                budget,
            ),
            PromptBlock::new(
                PromptAuthority::AppSystem,
                format!("app:{}", self.config.app_prompt.identity.app_id),
                self.app_system_context(),
                budget,
            ),
        ];
        if let Some(instructions) = &self.config.app_prompt.developer_instructions {
            blocks.push(PromptBlock::new(
                PromptAuthority::AppDeveloper,
                "app:developer",
                instructions.clone(),
                budget,
            ));
        }
        if let Some(instructions) = &self.config.developer_instructions {
            blocks.push(PromptBlock::new(
                PromptAuthority::AppDeveloper,
                "host:developer",
                format!("<developer_instructions>\n{instructions}\n</developer_instructions>"),
                budget,
            ));
        }
        blocks.push(PromptBlock::new(
            PromptAuthority::Workspace,
            "workspace:instructions",
            self.workspace_context(),
            budget,
        ));
        if let Some(instructions) = &self.config.session_instructions {
            blocks.push(PromptBlock::new(
                PromptAuthority::Session,
                "session:instructions",
                instructions.clone(),
                budget,
            ));
        }
        if let Some(context) = &self.config.memory_context {
            blocks.push(PromptBlock::new(
                PromptAuthority::Memory,
                "memory:recall",
                context.clone(),
                budget,
            ));
        }
        let skill_context = self.skill_context();
        if !skill_context.is_empty() {
            blocks.push(PromptBlock::new(
                PromptAuthority::Skill,
                "skills:snapshot",
                skill_context,
                budget,
            ));
        }
        if let Some(instructions) = &self.config.goal_instructions {
            blocks.push(PromptBlock::new(
                PromptAuthority::Goal,
                "goal:active",
                format!("<active_goal>\n{instructions}\n</active_goal>"),
                budget,
            ));
        }
        if let Some(instructions) = &self.config.turn_local_instructions {
            blocks.push(PromptBlock::new(
                PromptAuthority::TurnLocal,
                "turn:local",
                instructions.clone(),
                budget,
            ));
        }
        blocks
    }

    fn app_system_context(&self) -> String {
        let app = &self.config.app_prompt;
        let system_instructions = if self.config.base_instructions
            == crate::prompt_composer::DEFAULT_APP_SYSTEM_INSTRUCTIONS
        {
            &app.system_instructions
        } else {
            &self.config.base_instructions
        };
        let mut content = format!(
            "<agent_app id=\"{}\" version=\"{}\" display_name=\"{}\">\n",
            app.identity.app_id, app.identity.version, app.identity.display_name
        );
        if !app.identity.enabled_capabilities.is_empty() {
            content.push_str("enabled_capabilities=");
            content.push_str(&app.identity.enabled_capabilities.join(","));
            content.push('\n');
        }
        content.push_str(system_instructions);
        content.push_str("\n</agent_app>");
        content
    }

    fn workspace_context(&self) -> String {
        let mut context = String::from(
            "Use tools for concrete workspace actions. Respect the project instructions below in directory order.",
        );

        for document in &self.documents {
            context.push_str("\n\n");
            context.push_str(&format!(
                "<project_instructions source=\"{}\" bytes=\"{}\" original_bytes=\"{}\" truncated=\"{}\">\n",
                display_path(&document.path),
                document.read_bytes,
                document.original_bytes,
                document.truncated
            ));
            context.push_str(&document.content);
            context.push_str("\n</project_instructions>");
        }

        context
    }

    fn skill_context(&self) -> String {
        let mut context = String::new();

        if !self.config.skill_summaries.is_empty() {
            context.push_str(&format!(
                "<available_skills count=\"{}\">\n",
                self.config.skill_summaries.len()
            ));
            for summary in &self.config.skill_summaries {
                context.push_str(&format!("- name: {}\n", summary.name));
                context.push_str(&format!("  description: {}\n", summary.description));
                if !summary.aliases.is_empty() {
                    context.push_str(&format!("  aliases: {}\n", summary.aliases.join(", ")));
                }
                context.push_str(&format!("  source: {}\n", display_path(&summary.source)));
            }
            context.push_str("</available_skills>");
        }

        for document in &self.config.skill_instructions {
            if !context.is_empty() {
                context.push_str("\n\n");
            }
            context.push_str(&format!(
                "<skill_instructions name=\"{}\" source=\"{}\" bytes=\"{}\" original_bytes=\"{}\" truncated=\"{}\">\n",
                document.name,
                display_path(&document.source),
                document.read_bytes,
                document.original_bytes,
                document.truncated
            ));
            context.push_str(&document.content);
            context.push_str("\n</skill_instructions>");
        }

        context
    }
}

fn instruction_file_is_inside_workspace(workspace_root: &Path, path: &Path) -> bool {
    path.canonicalize().is_ok_and(|canonical_path| {
        canonical_path.is_file() && canonical_path.starts_with(workspace_root)
    })
}

fn instruction_dirs(workspace_root: &Path, cwd: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let relative_cwd = cwd.strip_prefix(workspace_root).with_context(|| {
        format!(
            "cwd {} must be inside workspace root {}",
            cwd.display(),
            workspace_root.display()
        )
    })?;
    let mut dirs = vec![workspace_root.to_path_buf()];
    let mut current = workspace_root.to_path_buf();

    for component in relative_cwd.components() {
        current.push(component.as_os_str());
        dirs.push(current.clone());
    }

    Ok(dirs)
}

fn read_instruction_document(
    workspace_root: &Path,
    path: &Path,
    max_bytes: usize,
) -> anyhow::Result<InstructionDocument> {
    let original_bytes = fs::metadata(path)
        .with_context(|| format!("read metadata for {}", path.display()))?
        .len() as usize;
    let truncated = original_bytes > max_bytes;
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut bytes = Vec::with_capacity(max_bytes.min(original_bytes));
    file.by_ref()
        .take(max_bytes as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;

    let content = String::from_utf8_lossy(&bytes).into_owned();
    let read_bytes = bytes.len();
    let relative_path = path
        .strip_prefix(workspace_root)
        .with_context(|| format!("make {} relative to workspace", path.display()))?
        .to_path_buf();

    Ok(InstructionDocument {
        path: relative_path,
        content,
        truncated,
        read_bytes,
        original_bytes,
    })
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agent-runtime-instructions-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn discovers_agents_files_from_workspace_to_cwd() {
        let root = temp_workspace();
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(root.join("AGENTS.md"), "root instructions").unwrap();
        fs::write(app_dir.join("AGENTS.md"), "app instructions").unwrap();

        let context = InstructionContext::load(InstructionConfig::new(&root, &app_dir)).unwrap();
        let paths: Vec<_> = context
            .documents()
            .iter()
            .map(|document| document.path.as_path())
            .collect();

        assert_eq!(
            paths,
            vec![PathBuf::from("AGENTS.md"), PathBuf::from("app/AGENTS.md")]
        );
    }

    #[test]
    fn renders_model_input_before_user_message() {
        let root = temp_workspace();
        fs::write(root.join("AGENTS.md"), "project rules").unwrap();
        let mut config = InstructionConfig::new(&root, &root);
        config.developer_instructions = Some("developer override".into());

        let context = InstructionContext::load(config).unwrap();
        let input = context.model_input("hello");

        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "system");
        assert!(input[0]["content"].as_str().unwrap().contains("AgentWeave"));
        assert_eq!(input[1]["role"], "developer");
        let developer = input[1]["content"].as_str().unwrap();
        assert!(developer.contains("Use tools"));
        assert!(developer.contains("developer override"));
        assert!(developer.contains("<project_instructions source=\"AGENTS.md\""));
        assert!(developer.contains("project rules"));
        assert_eq!(
            input[2],
            serde_json::json!({ "role": "user", "content": "hello" })
        );
    }

    #[test]
    fn renders_skill_summaries_and_selected_skill_instructions() {
        let root = temp_workspace();
        let mut config = InstructionConfig::new(&root, &root);
        config.skill_summaries = vec![crate::skill_catalog::SkillSummary {
            name: "planning".into(),
            description: "Write implementation plans.".into(),
            aliases: vec!["planner".into()],
            source: PathBuf::from("planning/SKILL.md"),
        }];
        config.skill_instructions = vec![crate::skill_catalog::SkillInstructionDocument {
            name: "planning".into(),
            source: PathBuf::from("planning/SKILL.md"),
            content: "# Planning\nUse checklists.".into(),
            truncated: false,
            read_bytes: 25,
            original_bytes: 25,
        }];

        let context = InstructionContext::load(config).unwrap();
        let input = context.model_input("use $planning");
        let developer = input[1]["content"].as_str().unwrap();

        assert!(developer.contains("<available_skills count=\"1\">"));
        assert!(developer.contains("name: planning"));
        assert!(developer.contains("aliases: planner"));
        assert!(developer.contains("<skill_instructions name=\"planning\""));
        assert!(developer.contains("# Planning"));
    }

    #[test]
    fn truncates_large_agents_file_with_metadata() {
        let root = temp_workspace();
        fs::write(root.join("AGENTS.md"), "abcdef").unwrap();
        let mut config = InstructionConfig::new(&root, &root);
        config.max_instruction_bytes = 3;

        let context = InstructionContext::load(config).unwrap();
        let document = &context.documents()[0];
        assert_eq!(document.content, "abc");
        assert!(document.truncated);

        let input = context.model_input("hello");
        let developer = input[1]["content"].as_str().unwrap();
        assert!(developer.contains("bytes=\"3\""));
        assert!(developer.contains("truncated=\"true\""));
        assert!(developer.contains("original_bytes=\"6\""));
    }

    #[test]
    fn rejects_cwd_outside_workspace() {
        let root = temp_workspace();
        let outside = temp_workspace();

        let result = InstructionContext::load(InstructionConfig::new(root, outside));

        assert!(result.is_err());
    }

    #[test]
    fn stores_nested_agents_paths_as_relative_and_stable() {
        let root = temp_workspace();
        let nested = root.join("app").join("feature");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("AGENTS.md"), "root").unwrap();
        fs::write(root.join("app").join("AGENTS.md"), "app").unwrap();
        fs::write(nested.join("AGENTS.md"), "feature").unwrap();

        let context = InstructionContext::load(InstructionConfig::new(&root, &nested)).unwrap();
        let paths: Vec<String> = context
            .documents()
            .iter()
            .map(|document| document.path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert_eq!(
            paths,
            vec!["AGENTS.md", "app/AGENTS.md", "app/feature/AGENTS.md"]
        );
    }
}
