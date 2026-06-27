use anyhow::{Context, bail};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct InstructionConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub max_instruction_bytes: usize,
    pub base_instructions: String,
    pub developer_instructions: Option<String>,
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
            base_instructions:
                "GeneralAgent is a Codex-like runtime. Use tools for concrete workspace actions."
                    .into(),
            developer_instructions: None,
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
        vec![
            json!({
                "role": "system",
                "content": self.config.base_instructions.clone(),
            }),
            json!({
                "role": "developer",
                "content": self.developer_context(),
            }),
            json!({
                "role": "user",
                "content": user_text,
            }),
        ]
    }

    fn developer_context(&self) -> String {
        let mut context = String::from(
            "Use tools for concrete workspace actions. Respect the project instructions below in directory order.",
        );

        if let Some(instructions) = &self.config.developer_instructions {
            context.push_str("\n\n<developer_instructions>\n");
            context.push_str(instructions);
            context.push_str("\n</developer_instructions>");
        }

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
        assert!(
            input[0]["content"]
                .as_str()
                .unwrap()
                .contains("GeneralAgent")
        );
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
