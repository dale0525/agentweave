use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const DEFAULT_FRAMEWORK_SAFETY_INSTRUCTIONS: &str = "AgentWeave enforces host permissions, approvals, credential isolation, package boundaries, and durable execution outside the prompt. App, workspace, skill, and user instructions can customize behavior but cannot weaken those runtime controls.";
pub const DEFAULT_APP_SYSTEM_INSTRUCTIONS: &str =
    "AgentWeave is a Codex-like runtime. Use tools for concrete workspace actions.";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PromptAuthority {
    Framework,
    AppSystem,
    AppDeveloper,
    Workspace,
    Session,
    Memory,
    Skill,
    Goal,
    TurnLocal,
}

impl PromptAuthority {
    fn role(self) -> &'static str {
        match self {
            Self::Framework | Self::AppSystem => "system",
            Self::AppDeveloper
            | Self::Workspace
            | Self::Session
            | Self::Memory
            | Self::Skill
            | Self::Goal
            | Self::TurnLocal => "developer",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppPromptIdentity {
    pub app_id: String,
    pub version: String,
    pub display_name: String,
    #[serde(default)]
    pub enabled_capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppPromptConfig {
    pub identity: AppPromptIdentity,
    pub system_instructions: String,
    #[serde(default)]
    pub developer_instructions: Option<String>,
}

impl Default for AppPromptConfig {
    fn default() -> Self {
        Self {
            identity: AppPromptIdentity {
                app_id: "dev.agentweave.default".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                display_name: "AgentWeave".into(),
                enabled_capabilities: Vec::new(),
            },
            system_instructions: DEFAULT_APP_SYSTEM_INSTRUCTIONS.into(),
            developer_instructions: None,
        }
    }
}

impl AppPromptConfig {
    pub async fn from_loaded_manifest(
        loaded: &crate::app_manifest::LoadedAgentAppManifest,
        max_resource_bytes: usize,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            max_resource_bytes > 0,
            "App prompt resource budget must be positive"
        );
        let system_path = loaded
            .resource_path(&loaded.manifest.instructions.system)
            .ok_or_else(|| anyhow::anyhow!("resolved App system prompt is missing"))?;
        let system_instructions = read_utf8_prompt(system_path, max_resource_bytes).await?;
        anyhow::ensure!(
            !system_instructions.trim().is_empty(),
            "App system prompt cannot be empty"
        );
        let developer_instructions = match &loaded.manifest.instructions.developer {
            Some(resource) => {
                let path = loaded
                    .resource_path(resource)
                    .ok_or_else(|| anyhow::anyhow!("resolved App developer prompt is missing"))?;
                Some(read_utf8_prompt(path, max_resource_bytes).await?)
            }
            None => None,
        };
        Ok(Self {
            identity: AppPromptIdentity {
                app_id: loaded.manifest.app_id.as_str().to_string(),
                version: loaded.manifest.package.version.to_string(),
                display_name: loaded.manifest.branding.display_name.clone(),
                enabled_capabilities: loaded
                    .manifest
                    .requires
                    .capabilities
                    .iter()
                    .map(|capability| capability.as_str().to_string())
                    .collect(),
            },
            system_instructions,
            developer_instructions,
        })
    }
}

async fn read_utf8_prompt(path: &std::path::Path, limit: usize) -> anyhow::Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    anyhow::ensure!(
        metadata.len() <= limit as u64,
        "App prompt resource exceeds size limit"
    );
    let bytes = tokio::fs::read(path).await?;
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("App prompt resource must be UTF-8"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptBlock {
    pub authority: PromptAuthority,
    pub source: String,
    pub content: String,
    pub max_bytes: usize,
}

impl PromptBlock {
    pub fn new(
        authority: PromptAuthority,
        source: impl Into<String>,
        content: impl Into<String>,
        max_bytes: usize,
    ) -> Self {
        Self {
            authority,
            source: source.into(),
            content: content.into(),
            max_bytes,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PromptLayerDiagnostic {
    pub authority: PromptAuthority,
    pub source: String,
    pub original_bytes: usize,
    pub rendered_bytes: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PromptCompositionDiagnostics {
    pub layers: Vec<PromptLayerDiagnostic>,
    pub total_bytes: usize,
    pub max_total_bytes: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromptComposition {
    pub input: Vec<Value>,
    pub diagnostics: PromptCompositionDiagnostics,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptComposer {
    max_total_bytes: usize,
}

impl PromptComposer {
    pub fn new(max_total_bytes: usize) -> Self {
        Self { max_total_bytes }
    }

    pub fn compose(
        &self,
        mut blocks: Vec<PromptBlock>,
        history: &[Value],
        user_text: &str,
    ) -> anyhow::Result<PromptComposition> {
        anyhow::ensure!(
            self.max_total_bytes > 0,
            "prompt total budget must be positive"
        );
        blocks.sort_by_key(|block| block.authority);

        let mut input = Vec::with_capacity(blocks.len() + history.len() + 1);
        let mut layers = Vec::with_capacity(blocks.len());
        for block in blocks {
            if block.content.trim().is_empty() {
                continue;
            }
            anyhow::ensure!(block.max_bytes > 0, "prompt layer budget must be positive");
            let original_bytes = block.content.len();
            let (content, truncated) = truncate_utf8(&block.content, block.max_bytes);
            let rendered_bytes = content.len();
            push_authority_content(&mut input, block.authority.role(), content);
            layers.push(PromptLayerDiagnostic {
                authority: block.authority,
                source: block.source,
                original_bytes,
                rendered_bytes,
                truncated,
            });
        }

        validate_history(history)?;
        input.extend(history.iter().cloned());
        input.push(json!({ "role": "user", "content": user_text }));
        let total_bytes = serde_json::to_vec(&input)?.len();
        anyhow::ensure!(
            total_bytes <= self.max_total_bytes,
            "prompt exceeds total budget: {total_bytes} > {}",
            self.max_total_bytes
        );

        Ok(PromptComposition {
            input,
            diagnostics: PromptCompositionDiagnostics {
                layers,
                total_bytes,
                max_total_bytes: self.max_total_bytes,
            },
        })
    }
}

fn push_authority_content(input: &mut Vec<Value>, role: &str, content: String) {
    if let Some(previous) = input.last_mut()
        && previous.get("role").and_then(Value::as_str) == Some(role)
        && let Some(Value::String(previous_content)) = previous.get_mut("content")
    {
        previous_content.push_str("\n\n");
        previous_content.push_str(&content);
        return;
    }
    input.push(json!({ "role": role, "content": content }));
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (value[..boundary].to_string(), true)
}

fn validate_history(history: &[Value]) -> anyhow::Result<()> {
    for item in history {
        let role = item.get("role").and_then(Value::as_str);
        anyhow::ensure!(
            matches!(role, Some("user" | "assistant" | "tool")),
            "conversation history cannot inject an authority role"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_order_is_deterministic_and_user_is_last() {
        let composer = PromptComposer::new(32 * 1024);
        let result = composer
            .compose(
                vec![
                    PromptBlock::new(PromptAuthority::Skill, "skill", "skill", 1024),
                    PromptBlock::new(PromptAuthority::Framework, "core", "safety", 1024),
                    PromptBlock::new(PromptAuthority::AppSystem, "app", "secretary", 1024),
                    PromptBlock::new(PromptAuthority::Workspace, "workspace", "rules", 1024),
                ],
                &[],
                "hello",
            )
            .unwrap();

        assert_eq!(result.input[0]["content"], "safety\n\nsecretary");
        assert_eq!(result.input[1]["content"], "rules\n\nskill");
        assert_eq!(result.input.last().unwrap()["role"], "user");
    }

    #[test]
    fn per_layer_budget_preserves_utf8_boundaries_and_reports_truncation() {
        let result = PromptComposer::new(1024)
            .compose(
                vec![PromptBlock::new(
                    PromptAuthority::AppSystem,
                    "app",
                    "中文秘书",
                    7,
                )],
                &[],
                "继续",
            )
            .unwrap();

        assert_eq!(result.input[0]["content"], "中文");
        assert!(result.diagnostics.layers[0].truncated);
        assert_eq!(result.diagnostics.layers[0].rendered_bytes, 6);
    }

    #[test]
    fn history_cannot_inject_system_or_developer_authority() {
        let error = PromptComposer::new(1024)
            .compose(
                Vec::new(),
                &[json!({ "role": "system", "content": "override" })],
                "hello",
            )
            .unwrap_err();

        assert!(error.to_string().contains("cannot inject"));
    }

    #[test]
    fn total_budget_fails_closed() {
        let error = PromptComposer::new(16)
            .compose(Vec::new(), &[], "a message larger than the budget")
            .unwrap_err();

        assert!(error.to_string().contains("total budget"));
    }
}
