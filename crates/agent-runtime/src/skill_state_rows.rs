use crate::skill_package::SkillPackageId;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::{Uuid, Version};

pub(crate) const REVISION_COLUMNS: &str = "revision_id, package_id, version, content_hash, storage_path, descriptor_json, validation_json, created_by, created_at, lifecycle_status";
pub(crate) const INSTALLATION_COLUMNS: &str = "package_id, source_layer, active_revision_id, enabled, trust_level, install_status, installed_at, updated_at";
pub(crate) const APPROVAL_COLUMNS: &str = "approval_id, package_id, revision_id, operation, requested_by, approved_by, status, permission_diff, created_at, resolved_at";
pub(crate) const SNAPSHOT_COLUMNS: &str =
    "generation, status, members_json, created_at, activated_at";
pub(crate) const AUDIT_COLUMNS: &str =
    "id, actor_id, operation, package_id, revision_id, result, metadata_json, created_at";
pub(crate) const CIRCUIT_COLUMNS: &str =
    "revision_id, consecutive_failures, open_until, updated_at";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillLayerRecord {
    Builtin,
    Managed,
    Session,
}

impl SkillLayerRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Managed => "managed",
            Self::Session => "session",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "builtin" => Ok(Self::Builtin),
            "managed" => Ok(Self::Managed),
            "session" => Ok(Self::Session),
            _ => anyhow::bail!("unknown skill layer: {value}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillInstallStatus {
    Active,
    Disabled,
    Inactive,
    Quarantined,
    Removed,
}

impl SkillInstallStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Inactive => "inactive",
            Self::Quarantined => "quarantined",
            Self::Removed => "removed",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            "inactive" => Ok(Self::Inactive),
            "quarantined" => Ok(Self::Quarantined),
            "removed" => Ok(Self::Removed),
            _ => anyhow::bail!("unknown skill install status: {value}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillRevisionStatus {
    Staging,
    Managed,
    Quarantined,
}

impl SkillRevisionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Managed => "managed",
            Self::Quarantined => "quarantined",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "staging" => Ok(Self::Staging),
            "managed" => Ok(Self::Managed),
            "quarantined" => Ok(Self::Quarantined),
            _ => anyhow::bail!("unknown skill revision status: {value}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

impl SkillApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            _ => anyhow::bail!("unknown skill approval status: {value}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSnapshotStatus {
    Candidate,
    Active,
    LastKnownGood,
}

impl SkillSnapshotStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Active => "active",
            Self::LastKnownGood => "last_known_good",
        }
    }

    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "active" => Ok(Self::Active),
            "last_known_good" => Ok(Self::LastKnownGood),
            _ => anyhow::bail!("unknown skill snapshot status: {value}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillRevisionRecord {
    pub revision_id: String,
    pub package_id: SkillPackageId,
    pub version: String,
    pub content_hash: String,
    pub storage_path: String,
    pub descriptor_json: Value,
    pub validation_json: Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub status: SkillRevisionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInstallationRecord {
    pub package_id: SkillPackageId,
    pub source_layer: SkillLayerRecord,
    pub active_revision_id: Option<String>,
    pub enabled: bool,
    pub trust_level: String,
    pub status: SkillInstallStatus,
    pub installed_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillApprovalRecord {
    pub approval_id: String,
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub operation: String,
    pub requested_by: String,
    // The Task 6 schema names this resolver identity `approved_by` for both outcomes.
    pub approved_by: Option<String>,
    pub status: SkillApprovalStatus,
    pub permission_diff: Value,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillSnapshotRecord {
    pub generation: u64,
    pub status: SkillSnapshotStatus,
    pub members_json: Value,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillAuditRecord {
    pub id: String,
    pub actor_id: String,
    pub operation: String,
    pub package_id: SkillPackageId,
    pub revision_id: Option<String>,
    pub result: String,
    pub metadata_json: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillCircuitStateRecord {
    pub revision_id: String,
    pub consecutive_failures: u64,
    pub open_until: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

pub struct NewSkillRevision {
    pub package_id: SkillPackageId,
    pub version: String,
    pub content_hash: String,
    pub storage_path: String,
    pub descriptor_json: Value,
    pub validation_json: Value,
    pub created_by: String,
}

pub struct NewSkillApproval {
    pub package_id: SkillPackageId,
    pub revision_id: String,
    pub operation: String,
    pub requested_by: String,
    pub permission_diff: Value,
}

pub(crate) fn revision_from_row(row: &SqliteRow) -> anyhow::Result<SkillRevisionRecord> {
    let revision_id: String = row.try_get("revision_id")?;
    validate_uuid_v4("revision_id", &revision_id)?;
    let status: String = row.try_get("lifecycle_status")?;
    let storage_path: String = row.try_get("storage_path")?;
    validate_storage_path(&storage_path)?;
    Ok(SkillRevisionRecord {
        revision_id,
        package_id: parse_package_id(row, "package_id")?,
        version: row.try_get("version")?,
        content_hash: row.try_get("content_hash")?,
        storage_path,
        descriptor_json: parse_row_json(row, "descriptor_json")?,
        validation_json: parse_row_json(row, "validation_json")?,
        created_by: row.try_get("created_by")?,
        created_at: parse_row_timestamp(row, "created_at")?,
        status: SkillRevisionStatus::parse(&status)?,
    })
}

pub(crate) fn installation_from_row(row: &SqliteRow) -> anyhow::Result<SkillInstallationRecord> {
    let layer: String = row.try_get("source_layer")?;
    let status: String = row.try_get("install_status")?;
    let enabled: i64 = row.try_get("enabled")?;
    let active_revision_id: Option<String> = row.try_get("active_revision_id")?;
    if let Some(revision_id) = &active_revision_id {
        validate_uuid_v4("active_revision_id", revision_id)?;
    }
    Ok(SkillInstallationRecord {
        package_id: parse_package_id(row, "package_id")?,
        source_layer: SkillLayerRecord::parse(&layer)?,
        active_revision_id,
        enabled: parse_bool("enabled", enabled)?,
        trust_level: row.try_get("trust_level")?,
        status: SkillInstallStatus::parse(&status)?,
        installed_at: parse_row_timestamp(row, "installed_at")?,
        updated_at: parse_row_timestamp(row, "updated_at")?,
    })
}

pub(crate) fn approval_from_row(row: &SqliteRow) -> anyhow::Result<SkillApprovalRecord> {
    let approval_id: String = row.try_get("approval_id")?;
    validate_uuid_v4("approval_id", &approval_id)?;
    let status: String = row.try_get("status")?;
    let status = SkillApprovalStatus::parse(&status)?;
    let approved_by: Option<String> = row.try_get("approved_by")?;
    let resolved_at = parse_optional_row_timestamp(row, "resolved_at")?;
    validate_approval_resolution_columns(status, approved_by.as_deref(), resolved_at.as_ref())?;
    Ok(SkillApprovalRecord {
        approval_id,
        package_id: parse_package_id(row, "package_id")?,
        revision_id: row.try_get("revision_id")?,
        operation: row.try_get("operation")?,
        requested_by: row.try_get("requested_by")?,
        approved_by,
        status,
        permission_diff: parse_row_json(row, "permission_diff")?,
        created_at: parse_row_timestamp(row, "created_at")?,
        resolved_at,
    })
}

fn validate_approval_resolution_columns(
    status: SkillApprovalStatus,
    approved_by: Option<&str>,
    resolved_at: Option<&DateTime<Utc>>,
) -> anyhow::Result<()> {
    match status {
        SkillApprovalStatus::Pending if approved_by.is_some() || resolved_at.is_some() => {
            anyhow::bail!("pending skill approval cannot have resolver or resolved_at")
        }
        SkillApprovalStatus::Approved | SkillApprovalStatus::Rejected
            if approved_by.is_none() || resolved_at.is_none() =>
        {
            anyhow::bail!("resolved skill approval requires resolver and resolved_at")
        }
        _ => Ok(()),
    }
}

pub(crate) fn snapshot_from_row(row: &SqliteRow) -> anyhow::Result<SkillSnapshotRecord> {
    let generation: i64 = row.try_get("generation")?;
    let status: String = row.try_get("status")?;
    Ok(SkillSnapshotRecord {
        generation: u64::try_from(generation).context("invalid negative snapshot generation")?,
        status: SkillSnapshotStatus::parse(&status)?,
        members_json: parse_row_json(row, "members_json")?,
        created_at: parse_row_timestamp(row, "created_at")?,
        activated_at: parse_optional_row_timestamp(row, "activated_at")?,
    })
}

pub(crate) fn audit_from_row(row: &SqliteRow) -> anyhow::Result<SkillAuditRecord> {
    let id: String = row.try_get("id")?;
    validate_uuid_v4("audit id", &id)?;
    Ok(SkillAuditRecord {
        id,
        actor_id: row.try_get("actor_id")?,
        operation: row.try_get("operation")?,
        package_id: parse_package_id(row, "package_id")?,
        revision_id: row.try_get("revision_id")?,
        result: row.try_get("result")?,
        metadata_json: parse_row_json(row, "metadata_json")?,
        created_at: parse_row_timestamp(row, "created_at")?,
    })
}

pub(crate) fn circuit_from_row(row: &SqliteRow) -> anyhow::Result<SkillCircuitStateRecord> {
    let revision_id: String = row.try_get("revision_id")?;
    validate_uuid_v4("revision_id", &revision_id)?;
    let failures: i64 = row.try_get("consecutive_failures")?;
    Ok(SkillCircuitStateRecord {
        revision_id,
        consecutive_failures: u64::try_from(failures)
            .context("invalid negative consecutive failure count")?,
        open_until: parse_optional_row_timestamp(row, "open_until")?,
        updated_at: parse_row_timestamp(row, "updated_at")?,
    })
}

pub(crate) fn validate_uuid_v4(field: &str, value: &str) -> anyhow::Result<()> {
    let parsed = Uuid::parse_str(value).with_context(|| format!("invalid UUID v4 in {field}"))?;
    if parsed.get_version() != Some(Version::Random) {
        anyhow::bail!("invalid UUID v4 in {field}: {value}");
    }
    Ok(())
}

pub(crate) fn validate_storage_path(path: &str) -> anyhow::Result<()> {
    if path.trim().is_empty() {
        anyhow::bail!("revision storage path cannot be empty");
    }
    Ok(())
}

fn parse_package_id(row: &SqliteRow, column: &str) -> anyhow::Result<SkillPackageId> {
    let value: String = row.try_get(column)?;
    SkillPackageId::parse(&value).with_context(|| format!("invalid {column}"))
}

fn parse_row_json(row: &SqliteRow, column: &str) -> anyhow::Result<Value> {
    let value: String = row.try_get(column)?;
    parse_json(column, &value)
}

pub(crate) fn parse_json(column: &str, value: &str) -> anyhow::Result<Value> {
    serde_json::from_str(value).with_context(|| format!("invalid JSON in {column}"))
}

fn parse_row_timestamp(row: &SqliteRow, column: &str) -> anyhow::Result<DateTime<Utc>> {
    let value: String = row.try_get(column)?;
    parse_timestamp(column, &value)
}

fn parse_optional_row_timestamp(
    row: &SqliteRow,
    column: &str,
) -> anyhow::Result<Option<DateTime<Utc>>> {
    let value: Option<String> = row.try_get(column)?;
    value
        .map(|value| parse_timestamp(column, &value))
        .transpose()
}

fn parse_timestamp(column: &str, value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid RFC3339 timestamp in {column}"))?
        .with_timezone(&Utc))
}

fn parse_bool(column: &str, value: i64) -> anyhow::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => anyhow::bail!("invalid boolean value in {column}: {value}"),
    }
}
