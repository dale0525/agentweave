use crate::events::RuntimeEvent;
use crate::platform::{CapabilitySet, PlatformId};
use crate::session::{Message, Session};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::storage::Storage;
use crate::tools::RuntimeConfig;
use crate::turn::{ModelClient, TurnRunner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileRuntimeInit {
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MobileTurnResult {
    pub assistant_text: String,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileRuntimeDiagnostics {
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
    pub built_in_tools_enabled: bool,
    pub registered_skill_tool_count: usize,
}

#[async_trait::async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve_secret(&self, secret_id: &str) -> anyhow::Result<Option<String>>;
}

pub struct MobileRuntimeHost<C> {
    storage: Storage,
    model: Arc<C>,
    skills: SkillRegistry,
    skill_catalog: SkillCatalog,
    runtime_config: RuntimeConfig,
    init: MobileRuntimeInit,
}

impl<C> MobileRuntimeHost<C>
where
    C: ModelClient,
{
    pub fn new_for_test(
        storage: Storage,
        model: C,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        runtime_config: RuntimeConfig,
        init: MobileRuntimeInit,
    ) -> Self {
        let runtime_config = mobile_safe_runtime_config(&init, runtime_config);
        let skills = mobile_safe_skill_registry(&init, skills);
        Self {
            storage,
            model: Arc::new(model),
            skills,
            skill_catalog,
            runtime_config,
            init,
        }
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        self.storage.create_session(title).await
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.storage.list_sessions().await
    }

    pub async fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        self.storage.list_messages(session_id).await
    }

    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.storage.delete_session(session_id).await
    }

    pub fn init(&self) -> &MobileRuntimeInit {
        &self.init
    }

    pub fn diagnostics(&self) -> MobileRuntimeDiagnostics {
        MobileRuntimeDiagnostics {
            platform: self.init.platform,
            capabilities: self.init.capabilities.clone(),
            built_in_tools_enabled: self.runtime_config.built_in_tools_enabled,
            registered_skill_tool_count: self.skills.tools().len(),
        }
    }

    pub async fn send_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> anyhow::Result<MobileTurnResult> {
        if !self.storage.session_exists(session_id).await? {
            anyhow::bail!("session not found");
        }
        let runner = TurnRunner::new_with_catalog_and_config(
            self.model.clone(),
            self.skills.clone(),
            self.skill_catalog.clone(),
            self.runtime_config.clone(),
        );
        let events = runner.run(content).await?;
        let assistant_text = assistant_text_from_events(&events);
        self.storage
            .append_turn(session_id, content, &assistant_text)
            .await?;
        Ok(MobileTurnResult {
            assistant_text,
            events,
        })
    }
}

fn mobile_safe_runtime_config(
    init: &MobileRuntimeInit,
    runtime_config: RuntimeConfig,
) -> RuntimeConfig {
    if init.platform == PlatformId::Android {
        runtime_config.without_builtin_tools()
    } else {
        runtime_config
    }
}

fn mobile_safe_skill_registry(init: &MobileRuntimeInit, skills: SkillRegistry) -> SkillRegistry {
    if init.platform == PlatformId::Android {
        skills.with_platform_capabilities(init.platform, init.capabilities.clone())
    } else {
        skills
    }
}

pub(crate) fn assistant_text_from_events(events: &[RuntimeEvent]) -> String {
    events
        .iter()
        .find_map(|event| {
            if let RuntimeEvent::AssistantMessageFinished { text } = event {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            events
                .iter()
                .filter_map(|event| {
                    if let RuntimeEvent::AssistantTextDelta { text } = event {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
}

#[async_trait::async_trait]
impl<C> ModelClient for Arc<C>
where
    C: ModelClient,
{
    async fn stream(
        &self,
        request: model_gateway::responses::GatewayRequest,
    ) -> anyhow::Result<crate::turn::ModelEventStream> {
        self.as_ref().stream(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapabilitySet, PlatformId};
    use crate::skill::SkillRegistry;
    use crate::skill_catalog::SkillCatalog;
    use crate::storage::Storage;
    use crate::tools::RuntimeConfig;
    use futures::stream;
    use model_gateway::responses::GatewayEvent;
    use serde_json::{Value, json};
    use std::path::Path;
    use tempfile::tempdir;

    struct FakeModel;

    #[async_trait::async_trait]
    impl crate::turn::ModelClient for FakeModel {
        async fn stream(
            &self,
            _request: model_gateway::responses::GatewayRequest,
        ) -> anyhow::Result<crate::turn::ModelEventStream> {
            Ok(Box::pin(stream::iter(vec![Ok(GatewayEvent::TextDelta {
                text: "hello from android".into(),
            })])))
        }
    }

    #[tokio::test]
    async fn mobile_host_persists_turn_messages() {
        let dir = tempdir().unwrap();
        let db_url = format!("sqlite://{}?mode=rwc", dir.path().join("ga.db").display());
        let storage = Storage::connect(&db_url).await.unwrap();
        let runtime_config =
            RuntimeConfig::workspace_write(dir.path(), dir.path()).without_builtin_tools();
        let host = MobileRuntimeHost::new_for_test(
            storage,
            FakeModel,
            SkillRegistry::empty(),
            SkillCatalog::empty(),
            runtime_config,
            MobileRuntimeInit {
                platform: PlatformId::Android,
                capabilities: CapabilitySet::android_mvp(),
            },
        );

        let session = host.create_session("Mobile").await.unwrap();
        let result = host.send_message(&session.id, "Hi").await.unwrap();
        let messages = host.get_messages(&session.id).await.unwrap();

        assert_eq!(result.assistant_text, "hello from android");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].content, "hello from android");
    }

    #[tokio::test]
    async fn android_host_disables_builtin_tools_even_for_workspace_write_config() {
        let dir = tempdir().unwrap();
        let db_url = format!("sqlite://{}?mode=rwc", dir.path().join("ga.db").display());
        let storage = Storage::connect(&db_url).await.unwrap();
        let runtime_config = RuntimeConfig::workspace_write(dir.path(), dir.path());

        let host = MobileRuntimeHost::new_for_test(
            storage,
            FakeModel,
            SkillRegistry::empty(),
            SkillCatalog::empty(),
            runtime_config,
            MobileRuntimeInit {
                platform: PlatformId::Android,
                capabilities: CapabilitySet::android_mvp(),
            },
        );

        assert!(!host.diagnostics().built_in_tools_enabled);
    }

    #[tokio::test]
    async fn android_host_hides_runtime_skill_tools_without_android_capability_support() {
        let dir = tempdir().unwrap();
        let skills_root = dir.path().join("skills");
        write_skill_manifest(
            &skills_root,
            "desktop-only",
            json!({
                "name": "desktop-only",
                "description": "Requires desktop automation.",
                "version": "0.1.0",
                "capabilities": {
                    "requires": ["browser.headless"]
                },
                "entry": { "type": "command", "command": "node", "args": ["index.js"] },
                "tools": [
                    {
                        "name": "desktop_only_tool",
                        "description": "Desktop only tool.",
                        "input_schema": { "type": "object" }
                    }
                ]
            }),
        )
        .await;
        tokio::fs::write(
            skills_root.join("desktop-only").join("index.js"),
            "process.stdin.resume();\n",
        )
        .await
        .unwrap();

        let db_url = format!("sqlite://{}?mode=rwc", dir.path().join("ga.db").display());
        let storage = Storage::connect(&db_url).await.unwrap();
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let host = MobileRuntimeHost::new_for_test(
            storage,
            FakeModel,
            skills,
            SkillCatalog::empty(),
            RuntimeConfig::workspace_write(dir.path(), dir.path()),
            MobileRuntimeInit {
                platform: PlatformId::Android,
                capabilities: CapabilitySet::android_mvp(),
            },
        );

        assert_eq!(host.diagnostics().registered_skill_tool_count, 0);
    }

    async fn write_skill_manifest(root: &Path, folder: &str, manifest: Value) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("skill.json"), manifest.to_string())
            .await
            .unwrap();
    }
}
