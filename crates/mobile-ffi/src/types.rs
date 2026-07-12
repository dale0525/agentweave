use agent_runtime::skill_policy::{ActorContext, SkillManagementPolicy};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileInitConfig {
    pub app_data_dir: String,
    pub cache_dir: String,
    pub database_path: String,
    pub builtin_skills_dir: String,
    pub managed_skills_dir: String,
    pub staging_skills_dir: String,
    pub quarantine_skills_dir: String,
    pub skill_policy: SkillManagementPolicy,
    pub actor_context: ActorContext,
    pub platform: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileDiagnostics {
    pub platform: String,
    pub capabilities: Vec<String>,
    pub database_ready: bool,
    pub skills_ready: bool,
    pub model_configured: bool,
    pub skill_management_mode: String,
    pub active_snapshot_generation: u64,
    pub quarantined_count: usize,
    pub last_reload_status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileSessionDto {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileMessageDto {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileSkillDto {
    pub package_id: String,
    pub display_name: String,
    pub version: String,
    pub source_layer: String,
    pub status: String,
    pub available: bool,
    pub reason: String,
    pub active_revision_id: Option<String>,
    pub manageable: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileModelConfigDto {
    pub provider_id: String,
    pub provider_name: String,
    pub endpoint_type: String,
    pub base_url: String,
    pub model_name: String,
    pub secret_id: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct MobileTurnDto {
    pub assistant_text: String,
}
