use crate::types::{MobileDiagnostics, MobileInitConfig};
use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::storage::Storage;
use anyhow::{Context, Result};
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
    pub fn initialize(config: MobileInitConfig) -> Result<Self> {
        let tokio = Runtime::new()?;
        let platform = parse_platform(&config.platform)?;
        let capabilities = CapabilitySet::from_names(config.capabilities);

        let app_data_dir = prepare_private_root(&config.app_data_dir)?;
        let cache_dir = prepare_private_root(&config.cache_dir)?;
        let allowed_roots = [app_data_dir.clone(), cache_dir.clone()];

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

    if allowed_roots.iter().any(|root| resolved_path.starts_with(root)) {
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
            std::path::Component::RootDir => {
                resolved.push(component.as_os_str());
            }
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
