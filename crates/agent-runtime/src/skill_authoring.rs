use crate::skill_management::{CreateSkillDraftRequest, DraftFileUpdate, SkillManagementError};
use crate::skill_package::{
    SKILL_PACKAGE_SCHEMA_VERSION, SkillCompatibility, SkillPackageDescriptor, SkillPackageKind,
    SkillPackageRequirements, SkillPackageTargets,
};
use crate::skill_store::StagingSkillFile;
use semver::Version;
use std::path::PathBuf;

const MAX_DRAFT_FILE_BYTES: usize = 256 * 1024;

pub fn validate_draft_updates(
    updates: Vec<DraftFileUpdate>,
) -> Result<Vec<StagingSkillFile>, SkillManagementError> {
    if updates.is_empty() {
        return Err(SkillManagementError::InvalidRequest(
            "draft update must contain at least one file".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::with_capacity(updates.len());
    for update in updates {
        if !allowed_draft_path(&update.path) {
            return Err(SkillManagementError::InvalidRequest(format!(
                "draft path is not allowed: {}",
                update.path.display()
            )));
        }
        if !seen.insert(update.path.clone()) {
            return Err(SkillManagementError::InvalidRequest(format!(
                "duplicate draft path: {}",
                update.path.display()
            )));
        }
        if update.content.len() > MAX_DRAFT_FILE_BYTES {
            return Err(SkillManagementError::InvalidRequest(format!(
                "draft file exceeds 256 KiB: {}",
                update.path.display()
            )));
        }
        files.push(StagingSkillFile {
            path: update.path,
            bytes: update.content.into_bytes(),
        });
    }
    Ok(files)
}

fn allowed_draft_path(path: &std::path::Path) -> bool {
    let mut components = path.components();
    let Some(std::path::Component::Normal(first)) = components.next() else {
        return false;
    };
    if first.is_empty()
        || components
            .clone()
            .any(|component| !matches!(component, std::path::Component::Normal(name) if !name.is_empty()))
    {
        return false;
    }
    match first.to_str() {
        Some("general-agent.json" | "SKILL.md") => components.next().is_none(),
        Some("references" | "assets") => components.next().is_some(),
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredSkillPackage {
    files: Vec<StagingSkillFile>,
}

impl AuthoredSkillPackage {
    pub fn files(&self) -> &[StagingSkillFile] {
        &self.files
    }

    pub fn descriptor_bytes(&self) -> &[u8] {
        &self.files[0].bytes
    }

    pub fn instructions_bytes(&self) -> &[u8] {
        &self.files[1].bytes
    }
}

pub fn build_package_draft(
    request: &CreateSkillDraftRequest,
) -> Result<AuthoredSkillPackage, SkillManagementError> {
    let display_name = normalize_display_name(&request.display_name)?;
    let description = normalize_description(&request.description)?;
    let required_tools = normalize_required_tools(&request.required_tools)?;
    validate_kind_requirements(request.kind, &required_tools)?;

    let descriptor = SkillPackageDescriptor {
        schema_version: SKILL_PACKAGE_SCHEMA_VERSION,
        id: request.package_id.clone(),
        version: Version::new(0, 1, 0),
        display_name: display_name.clone(),
        kind: request.kind,
        package: SkillPackageTargets {
            include_instructions: true,
            include_runtime: false,
        },
        compatibility: SkillCompatibility::default(),
        requires: SkillPackageRequirements {
            runtime_tools: required_tools,
            ..SkillPackageRequirements::default()
        },
    };
    descriptor
        .validate()
        .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;

    let mut descriptor_bytes = serde_json::to_vec_pretty(&descriptor)
        .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
    descriptor_bytes.push(b'\n');
    let yaml_description = serde_json::to_string(&description)
        .map_err(|error| SkillManagementError::InvalidRequest(error.to_string()))?;
    let skill_name = request.package_id.as_str().replace('.', "-");
    let heading = escape_markdown_heading(&display_name);
    let body = escape_markdown_body(&description);
    let instructions = format!(
        "---\nname: {skill_name}\ndescription: {yaml_description}\n---\n\n# {heading}\n\n{body}\n"
    )
    .into_bytes();

    Ok(AuthoredSkillPackage {
        files: vec![
            StagingSkillFile {
                path: PathBuf::from("general-agent.json"),
                bytes: descriptor_bytes,
            },
            StagingSkillFile {
                path: PathBuf::from("SKILL.md"),
                bytes: instructions,
            },
        ],
    })
}

fn normalize_display_name(value: &str) -> Result<String, SkillManagementError> {
    reject_nul(value, "display name")?;
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return Err(SkillManagementError::InvalidRequest(
            "display name cannot be empty".into(),
        ));
    }
    Ok(normalized)
}

fn normalize_description(value: &str) -> Result<String, SkillManagementError> {
    reject_nul(value, "description")?;
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim().to_string();
    if normalized.is_empty() {
        return Err(SkillManagementError::InvalidRequest(
            "description cannot be empty".into(),
        ));
    }
    Ok(normalized)
}

fn normalize_required_tools(values: &[String]) -> Result<Vec<String>, SkillManagementError> {
    let mut tools = Vec::with_capacity(values.len());
    for value in values {
        let tool = value.trim();
        let valid = !tool.is_empty() && tool.len() <= 64 && valid_required_tool(tool);
        if !valid {
            return Err(SkillManagementError::InvalidRequest(format!(
                "invalid required tool: {value}"
            )));
        }
        tools.push(tool.to_string());
    }
    tools.sort();
    tools.dedup();
    Ok(tools)
}

fn valid_required_tool(tool: &str) -> bool {
    let valid_leaf = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    };
    if valid_leaf(tool) {
        return true;
    }
    let Some((namespace, leaf)) = tool.split_once('/') else {
        return false;
    };
    !leaf.contains('/')
        && valid_leaf(leaf)
        && namespace.split('.').count() >= 3
        && namespace.split('.').all(valid_leaf)
}

fn validate_kind_requirements(
    kind: SkillPackageKind,
    required_tools: &[String],
) -> Result<(), SkillManagementError> {
    match kind {
        SkillPackageKind::InstructionOnly if required_tools.is_empty() => Ok(()),
        SkillPackageKind::InstructionOnly => Err(SkillManagementError::InvalidRequest(
            "instruction-only drafts cannot require runtime tools".into(),
        )),
        SkillPackageKind::HostToolsOnly if !required_tools.is_empty() => Ok(()),
        SkillPackageKind::HostToolsOnly => Err(SkillManagementError::InvalidRequest(
            "host-tools-only drafts require at least one runtime tool".into(),
        )),
        SkillPackageKind::NativeRuntime => Err(SkillManagementError::InvalidRequest(
            "native runtime authoring is disabled".into(),
        )),
    }
}

fn reject_nul(value: &str, label: &str) -> Result<(), SkillManagementError> {
    if value.contains('\0') {
        return Err(SkillManagementError::InvalidRequest(format!(
            "{label} contains a NUL byte"
        )));
    }
    Ok(())
}

fn escape_markdown_heading(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if matches!(
                ch,
                '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '#'
            ) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}

fn escape_markdown_body(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            if line.trim() == "---" {
                line.replacen("---", "\\---", 1)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
