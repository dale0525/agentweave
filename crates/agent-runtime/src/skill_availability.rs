use crate::platform::{CapabilitySet, PlatformId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillCapabilityMetadata {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
    #[serde(default)]
    pub platforms: PlatformOverrides,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlatformOverrides {
    #[serde(default)]
    pub android: Option<PlatformSkillOverride>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlatformSkillOverride {
    pub status: PlatformSkillStatus,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformSkillStatus {
    Available,
    Unsupported,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillAvailabilityStatus {
    Available,
    Unavailable,
    Unsupported,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillAvailability {
    pub skill_id: String,
    pub status: SkillAvailabilityStatus,
    pub missing_capabilities: Vec<String>,
    pub reason: String,
}

pub fn evaluate_skill_availability(
    skill_id: &str,
    metadata: &SkillCapabilityMetadata,
    platform: PlatformId,
    capabilities: &CapabilitySet,
    contributes_runtime_tools: bool,
) -> SkillAvailability {
    if let Some(override_status) = platform_override(metadata, platform) {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: match override_status.status {
                PlatformSkillStatus::Available => SkillAvailabilityStatus::Available,
                PlatformSkillStatus::Unavailable => SkillAvailabilityStatus::Unavailable,
                PlatformSkillStatus::Unsupported => SkillAvailabilityStatus::Unsupported,
            },
            missing_capabilities: Vec::new(),
            reason: override_status.reason.clone(),
        };
    }

    if platform == PlatformId::Android
        && contributes_runtime_tools
        && metadata.requires.is_empty()
        && metadata.optional.is_empty()
    {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: SkillAvailabilityStatus::Unavailable,
            missing_capabilities: Vec::new(),
            reason: "Runtime tools must declare capability requirements on Android.".into(),
        };
    }

    let missing: Vec<String> = metadata
        .requires
        .iter()
        .filter(|name| !capabilities.contains_name(name))
        .cloned()
        .collect();

    if missing.is_empty() {
        return SkillAvailability {
            skill_id: skill_id.to_string(),
            status: SkillAvailabilityStatus::Available,
            missing_capabilities: Vec::new(),
            reason: "Available on this platform.".into(),
        };
    }

    SkillAvailability {
        skill_id: skill_id.to_string(),
        status: SkillAvailabilityStatus::Unavailable,
        reason: if missing.len() == 1 {
            format!("Missing required capability: {}", missing[0])
        } else {
            format!("Missing required capabilities: {}", missing.join(", "))
        },
        missing_capabilities: missing,
    }
}

fn platform_override(
    metadata: &SkillCapabilityMetadata,
    platform: PlatformId,
) -> Option<&PlatformSkillOverride> {
    match platform {
        PlatformId::Android => metadata.platforms.android.as_ref(),
        PlatformId::Desktop | PlatformId::Ios | PlatformId::Web | PlatformId::Server => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapabilitySet, PlatformId};

    #[test]
    fn disables_skill_when_required_capability_is_missing() {
        let metadata = SkillCapabilityMetadata {
            requires: vec!["network.http".into(), "browser.headless".into()],
            optional: vec![],
            platforms: PlatformOverrides::default(),
        };
        let availability = evaluate_skill_availability(
            "web-search",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            false,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unavailable);
        assert_eq!(availability.missing_capabilities, vec!["browser.headless"]);
        assert_eq!(
            availability.reason,
            "Missing required capability: browser.headless"
        );
    }

    #[test]
    fn platform_override_uses_human_reason() {
        let metadata = SkillCapabilityMetadata {
            requires: vec!["network.http".into()],
            optional: vec![],
            platforms: PlatformOverrides {
                android: Some(PlatformSkillOverride {
                    status: PlatformSkillStatus::Unsupported,
                    reason: "Requires a desktop headless browser.".into(),
                }),
            },
        };
        let availability = evaluate_skill_availability(
            "web-search",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            false,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unsupported);
        assert_eq!(availability.reason, "Requires a desktop headless browser.");
    }

    #[test]
    fn android_disables_runtime_tools_without_metadata() {
        let metadata = SkillCapabilityMetadata::default();
        let availability = evaluate_skill_availability(
            "legacy-tool-skill",
            &metadata,
            PlatformId::Android,
            &CapabilitySet::android_mvp(),
            true,
        );

        assert_eq!(availability.status, SkillAvailabilityStatus::Unavailable);
        assert_eq!(
            availability.reason,
            "Runtime tools must declare capability requirements on Android."
        );
    }
}
