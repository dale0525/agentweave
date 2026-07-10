use crate::types::{
    MobileDiagnostics, MobileInitConfig, MobileMessageDto, MobileModelConfigDto, MobileSessionDto,
    MobileTurnDto,
};
use agent_runtime::mobile_host::{HttpMobileRuntimeHost, MobileRuntimeInit, SecretResolver};
use agent_runtime::model_config::StoredModelConfig;
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill::SkillRegistry;
use agent_runtime::skill_catalog::SkillCatalog;
use agent_runtime::storage::Storage;
use agent_runtime::tools::RuntimeConfig;
use anyhow::{Context, Result};
use model_gateway::provider::EndpointType;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

pub struct MobileRuntime {
    tokio: Runtime,
    storage: Storage,
    init: MobileRuntimeInit,
    skills: SkillRegistry,
    skill_catalog: SkillCatalog,
    runtime_config: RuntimeConfig,
    database_ready: bool,
    skills_ready: bool,
    model_configured: AtomicBool,
    cancellation: CancellationToken,
}

impl MobileRuntime {
    pub fn initialize(config: MobileInitConfig) -> Result<Self> {
        let tokio = Runtime::new()?;
        let platform = parse_platform(&config.platform)?;
        let capabilities = CapabilitySet::from_names(config.capabilities);
        let app_data_dir = prepare_private_root(&config.app_data_dir)?;
        let cache_dir = prepare_private_root(&config.cache_dir)?;
        let allowed_roots = [app_data_dir.clone(), cache_dir];
        let skills_path = resolve_private_path(
            &config.skills_dir,
            &app_data_dir,
            &allowed_roots,
            "skills directory",
        )?;
        std::fs::create_dir_all(&skills_path)?;
        let database_path = resolve_private_path(
            &config.database_path,
            &app_data_dir,
            &allowed_roots,
            "database path",
        )?;
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
        let storage = tokio.block_on(Storage::connect(&database_url))?;
        let skills = tokio.block_on(SkillRegistry::load_development(&skills_path))?;
        let skill_catalog = tokio.block_on(SkillCatalog::load_development(&skills_path))?;
        let model_configured = tokio.block_on(storage.load_model_config())?.is_some();
        let runtime_config = RuntimeConfig::workspace_write(&app_data_dir, &app_data_dir);

        Ok(Self {
            tokio,
            storage,
            init: MobileRuntimeInit {
                platform,
                capabilities,
            },
            skills,
            skill_catalog,
            runtime_config,
            database_ready: true,
            skills_ready: skills_path.is_dir(),
            model_configured: AtomicBool::new(model_configured),
            cancellation: CancellationToken::new(),
        })
    }

    pub fn diagnostics(&self) -> MobileDiagnostics {
        MobileDiagnostics {
            platform: platform_name(self.init.platform).to_string(),
            capabilities: self.init.capabilities.names().to_vec(),
            database_ready: self.database_ready,
            skills_ready: self.skills_ready,
            model_configured: self.model_configured.load(Ordering::Acquire),
        }
    }

    pub fn create_session(&self, title: &str) -> Result<MobileSessionDto> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("session title is required");
        }
        self.tokio
            .block_on(self.storage.create_session(title))
            .map(Into::into)
    }

    pub fn list_sessions(&self) -> Result<Vec<MobileSessionDto>> {
        self.tokio
            .block_on(self.storage.list_sessions())
            .map(|sessions| sessions.into_iter().map(Into::into).collect())
    }

    pub fn get_messages(&self, session_id: &str) -> Result<Vec<MobileMessageDto>> {
        self.tokio
            .block_on(self.storage.list_messages(session_id))
            .map(|messages| messages.into_iter().map(Into::into).collect())
    }

    pub fn delete_session(&self, session_id: &str) -> Result<()> {
        self.tokio.block_on(self.storage.delete_session(session_id))
    }

    pub fn save_model_config(&self, config: MobileModelConfigDto) -> Result<()> {
        let stored = StoredModelConfig::try_from(config)?;
        self.tokio
            .block_on(self.storage.save_model_config(&stored))?;
        self.model_configured.store(true, Ordering::Release);
        Ok(())
    }

    pub fn load_model_config(&self) -> Result<Option<MobileModelConfigDto>> {
        self.tokio
            .block_on(self.storage.load_model_config())
            .map(|config| config.map(Into::into))
    }

    pub fn send_message(
        &self,
        session_id: &str,
        content: &str,
        api_key: Option<String>,
    ) -> Result<MobileTurnDto> {
        let config = self
            .tokio
            .block_on(self.storage.load_model_config())?
            .ok_or_else(|| anyhow::anyhow!("model configuration is required"))?;
        if config.secret_id.is_some()
            && api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            anyhow::bail!("model API key is unavailable");
        }
        let host = HttpMobileRuntimeHost::new(
            self.storage.clone(),
            self.skills.clone(),
            self.skill_catalog.clone(),
            self.runtime_config.clone(),
            self.init.clone(),
            config,
            TransientSecretResolver::new(api_key),
        );
        let cancellation = self.cancellation.clone();
        let result = self.tokio.block_on(async {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => anyhow::bail!("runtime closed"),
                result = tokio::time::timeout(
                    Duration::from_secs(60),
                    host.send_message(session_id, content),
                ) => match result {
                    Ok(result) => result,
                    Err(_) => anyhow::bail!("model turn timed out"),
                },
            }
        })?;
        Ok(MobileTurnDto {
            assistant_text: result.assistant_text,
        })
    }

    pub fn close(&self) {
        self.cancellation.cancel();
    }
}

struct TransientSecretResolver {
    secret: Mutex<Option<String>>,
}

impl TransientSecretResolver {
    fn new(secret: Option<String>) -> Self {
        Self {
            secret: Mutex::new(secret),
        }
    }
}

#[async_trait::async_trait]
impl SecretResolver for TransientSecretResolver {
    async fn resolve_secret(&self, _secret_id: &str) -> Result<Option<String>> {
        Ok(self
            .secret
            .lock()
            .map_err(|_| anyhow::anyhow!("model secret lock is unavailable"))?
            .take())
    }
}

impl From<agent_runtime::session::Session> for MobileSessionDto {
    fn from(session: agent_runtime::session::Session) -> Self {
        Self {
            id: session.id,
            title: session.title,
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.updated_at.to_rfc3339(),
        }
    }
}

impl From<agent_runtime::session::Message> for MobileMessageDto {
    fn from(message: agent_runtime::session::Message) -> Self {
        Self {
            id: message.id,
            session_id: message.session_id,
            role: message.role,
            content: message.content,
            created_at: message.created_at.to_rfc3339(),
        }
    }
}

impl TryFrom<MobileModelConfigDto> for StoredModelConfig {
    type Error = anyhow::Error;

    fn try_from(config: MobileModelConfigDto) -> Result<Self> {
        let endpoint_type = match config.endpoint_type.as_str() {
            "responses" => EndpointType::Responses,
            "chat_completions" => EndpointType::ChatCompletions,
            "completion" => EndpointType::Completion,
            value => anyhow::bail!("unsupported endpoint type: {value}"),
        };
        let stored = Self {
            provider_id: config.provider_id,
            provider_name: config.provider_name,
            endpoint_type,
            base_url: config.base_url,
            model_name: config.model_name,
            secret_id: config.secret_id,
            headers: config.headers,
        };
        stored.validate().map_err(anyhow::Error::msg)?;
        Ok(stored)
    }
}

impl From<StoredModelConfig> for MobileModelConfigDto {
    fn from(config: StoredModelConfig) -> Self {
        Self {
            provider_id: config.provider_id,
            provider_name: config.provider_name,
            endpoint_type: match config.endpoint_type {
                EndpointType::Responses => "responses",
                EndpointType::ChatCompletions => "chat_completions",
                EndpointType::Completion => "completion",
            }
            .into(),
            base_url: config.base_url,
            model_name: config.model_name,
            secret_id: config.secret_id,
            headers: config.headers,
        }
    }
}

fn parse_platform(value: &str) -> Result<PlatformId> {
    match value {
        "android" => Ok(PlatformId::Android),
        "desktop" => Ok(PlatformId::Desktop),
        "ios" => Ok(PlatformId::Ios),
        "web" => Ok(PlatformId::Web),
        "server" => Ok(PlatformId::Server),
        _ => anyhow::bail!("unsupported platform: {value}"),
    }
}

fn platform_name(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Android => "android",
        PlatformId::Desktop => "desktop",
        PlatformId::Ios => "ios",
        PlatformId::Web => "web",
        PlatformId::Server => "server",
    }
}

fn prepare_private_root(path: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(path)?;
    Path::new(path)
        .canonicalize()
        .with_context(|| format!("failed to canonicalize app-private root: {path}"))
}

fn resolve_private_path(
    raw_path: &str,
    default_root: &Path,
    allowed_roots: &[PathBuf],
    label: &str,
) -> Result<PathBuf> {
    let candidate = Path::new(raw_path);
    let absolute_candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        default_root.join(candidate)
    };
    let resolved_path = canonicalize_existing_ancestors(&absolute_candidate)?;

    if allowed_roots
        .iter()
        .any(|root| resolved_path.starts_with(root))
    {
        Ok(resolved_path)
    } else {
        anyhow::bail!("{label} must stay inside app-private storage")
    }
}

fn canonicalize_existing_ancestors(path: &Path) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            std::path::Component::RootDir => resolved.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                anyhow::bail!("path must stay inside app-private storage")
            }
            std::path::Component::Normal(part) => {
                let next = resolved.join(part);
                if next.exists() {
                    resolved = next.canonicalize().with_context(|| {
                        format!("failed to canonicalize existing path: {}", next.display())
                    })?;
                } else {
                    resolved = next;
                }
            }
        }
    }

    Ok(resolved)
}
