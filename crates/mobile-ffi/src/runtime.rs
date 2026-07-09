use crate::types::{MobileDiagnostics, MobileInitConfig};
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::storage::Storage;
use std::path::{Path, PathBuf};
use tokio::runtime::Runtime;

pub struct MobileRuntime {
    _tokio: Runtime,
    platform: PlatformId,
    capabilities: CapabilitySet,
    database_ready: bool,
    skills_ready: bool,
}

impl MobileRuntime {
    pub fn initialize(config: MobileInitConfig) -> anyhow::Result<Self> {
        let tokio = Runtime::new()?;
        let platform = parse_platform(&config.platform)?;
        let capabilities = CapabilitySet::from_names(config.capabilities);

        std::fs::create_dir_all(&config.app_data_dir)?;
        std::fs::create_dir_all(&config.cache_dir)?;

        let skills_path = resolve_skills_path(&config.app_data_dir, &config.skills_dir);
        std::fs::create_dir_all(&skills_path)?;

        let database_path = PathBuf::from(&config.database_path);
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let database_url = format!("sqlite://{}?mode=rwc", database_path.display());
        tokio.block_on(Storage::connect(&database_url))?;

        Ok(Self {
            _tokio: tokio,
            platform,
            capabilities,
            database_ready: true,
            skills_ready: skills_path.is_dir(),
        })
    }

    pub fn diagnostics(&self) -> MobileDiagnostics {
        MobileDiagnostics {
            platform: platform_name(self.platform).to_string(),
            capabilities: self.capabilities.names().to_vec(),
            database_ready: self.database_ready,
            skills_ready: self.skills_ready,
            model_configured: false,
        }
    }
}

fn parse_platform(value: &str) -> anyhow::Result<PlatformId> {
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

fn resolve_skills_path(app_data_dir: &str, skills_dir: &str) -> PathBuf {
    let skills_path = Path::new(skills_dir);
    if skills_path.is_absolute() {
        skills_path.to_path_buf()
    } else {
        Path::new(app_data_dir).join(skills_path)
    }
}
