use crate::skill_package::SkillPackageId;
use crate::storage::Storage;
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

const REVISION_COLUMNS: &str = "revision_id, package_id, version, content_hash, storage_path, descriptor_json, validation_json, created_by, created_at";
const INSTALLATION_COLUMNS: &str = "package_id, source_layer, active_revision_id, enabled, trust_level, install_status, installed_at, updated_at";
const APPROVAL_COLUMNS: &str = "approval_id, package_id, revision_id, operation, requested_by, approved_by, status, permission_diff, created_at, resolved_at";
const AUDIT_COLUMNS: &str =
    "id, actor_id, operation, package_id, revision_id, result, metadata_json, created_at";

#[derive(Clone)]
pub struct SkillStateStore {
    storage: Storage,
}

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

    fn parse(value: &str) -> anyhow::Result<Self> {
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

    fn parse(value: &str) -> anyhow::Result<Self> {
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

    fn parse(value: &str) -> anyhow::Result<Self> {
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

pub(crate) async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    for statement in [
        r#"CREATE TABLE IF NOT EXISTS skill_installations (
          package_id TEXT PRIMARY KEY,
          source_layer TEXT NOT NULL CHECK(source_layer IN ('builtin', 'managed', 'session')),
          active_revision_id TEXT,
          enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
          trust_level TEXT NOT NULL CHECK(length(trust_level) > 0),
          install_status TEXT NOT NULL CHECK(install_status IN ('active', 'disabled', 'inactive', 'quarantined', 'removed')),
          installed_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_revisions (
          revision_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          version TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          storage_path TEXT NOT NULL,
          descriptor_json TEXT NOT NULL,
          validation_json TEXT NOT NULL,
          created_by TEXT NOT NULL,
          created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_approvals (
          approval_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          revision_id TEXT NOT NULL,
          operation TEXT NOT NULL,
          requested_by TEXT NOT NULL,
          approved_by TEXT,
          status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected')),
          permission_diff TEXT NOT NULL,
          created_at TEXT NOT NULL,
          resolved_at TEXT
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_snapshots (
          generation INTEGER PRIMARY KEY CHECK(generation >= 0),
          status TEXT NOT NULL CHECK(status IN ('candidate', 'active', 'last_known_good')),
          members_json TEXT NOT NULL,
          created_at TEXT NOT NULL,
          activated_at TEXT
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_audit_log (
          id TEXT PRIMARY KEY,
          actor_id TEXT NOT NULL,
          operation TEXT NOT NULL,
          package_id TEXT NOT NULL,
          revision_id TEXT,
          result TEXT NOT NULL,
          metadata_json TEXT NOT NULL,
          created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_circuit_state (
          revision_id TEXT PRIMARY KEY,
          consecutive_failures INTEGER NOT NULL CHECK(consecutive_failures >= 0),
          open_until TEXT,
          updated_at TEXT NOT NULL
        )"#,
        "CREATE INDEX IF NOT EXISTS idx_skill_revisions_package ON skill_revisions(package_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_skill_installations_active ON skill_installations(enabled, install_status)",
        "CREATE INDEX IF NOT EXISTS idx_skill_approvals_package_status ON skill_approvals(package_id, status, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_skill_audit_package_created ON skill_audit_log(package_id, created_at, id)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_snapshots_single_active ON skill_snapshots(status) WHERE status = 'active'",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_snapshots_single_lkg ON skill_snapshots(status) WHERE status = 'last_known_good'",
    ] {
        sqlx::query(statement).execute(pool).await?;
    }
    Ok(())
}

impl SkillStateStore {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    pub async fn create_revision(
        &self,
        input: NewSkillRevision,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let revision_id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let descriptor_json = serde_json::to_string(&input.descriptor_json)?;
        let validation_json = serde_json::to_string(&input.validation_json)?;
        sqlx::query(
            r#"INSERT INTO skill_revisions
               (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
                validation_json, created_by, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&revision_id)
        .bind(input.package_id.as_str())
        .bind(&input.version)
        .bind(&input.content_hash)
        .bind(&input.storage_path)
        .bind(descriptor_json)
        .bind(validation_json)
        .bind(&input.created_by)
        .bind(created_at.to_rfc3339())
        .execute(self.storage.pool())
        .await?;

        Ok(SkillRevisionRecord {
            revision_id,
            package_id: input.package_id,
            version: input.version,
            content_hash: input.content_hash,
            storage_path: input.storage_path,
            descriptor_json: input.descriptor_json,
            validation_json: input.validation_json,
            created_by: input.created_by,
            created_at,
        })
    }

    pub async fn get_revision(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<Option<SkillRevisionRecord>> {
        let query = format!("SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE revision_id = ?");
        sqlx::query(&query)
            .bind(revision_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| revision_from_row(&row))
            .transpose()
    }

    pub async fn revision_validation(&self, revision_id: &str) -> anyhow::Result<Value> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT validation_json FROM skill_revisions WHERE revision_id = ?")
                .bind(revision_id)
                .fetch_optional(self.storage.pool())
                .await?;
        let value = value.with_context(|| format!("skill revision not found: {revision_id}"))?;
        parse_json("validation_json", &value)
    }

    pub async fn update_revision_validation(
        &self,
        revision_id: &str,
        value: Value,
    ) -> anyhow::Result<()> {
        let value = serde_json::to_string(&value)?;
        let result =
            sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
                .bind(value)
                .bind(revision_id)
                .execute(self.storage.pool())
                .await?;
        ensure_changed(result.rows_affected(), "skill revision", revision_id)
    }

    pub async fn get_installation(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Option<SkillInstallationRecord>> {
        let query =
            format!("SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?");
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| installation_from_row(&row))
            .transpose()
    }

    pub async fn list_active_installations(&self) -> anyhow::Result<Vec<SkillInstallationRecord>> {
        let query = format!(
            "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE enabled = 1 AND install_status = 'active' ORDER BY package_id"
        );
        sqlx::query(&query)
            .fetch_all(self.storage.pool())
            .await?
            .iter()
            .map(installation_from_row)
            .collect()
    }

    pub async fn activate_revision(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        layer: SkillLayerRecord,
        actor_id: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now().to_rfc3339();
        let mut tx = self.storage.pool().begin().await?;
        let stored_package: Option<String> =
            sqlx::query_scalar("SELECT package_id FROM skill_revisions WHERE revision_id = ?")
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?;
        let stored_package =
            stored_package.with_context(|| format!("skill revision not found: {revision_id}"))?;
        let stored_package = SkillPackageId::parse(&stored_package)?;
        if &stored_package != package_id {
            anyhow::bail!(
                "skill revision {revision_id} belongs to {}, not {}",
                stored_package.as_str(),
                package_id.as_str()
            );
        }

        sqlx::query(
            r#"INSERT INTO skill_installations
               (package_id, source_layer, active_revision_id, enabled, trust_level,
                install_status, installed_at, updated_at)
               VALUES (?, ?, ?, 1, 'approved', 'active', ?, ?)
               ON CONFLICT(package_id) DO UPDATE SET
                 source_layer = excluded.source_layer,
                 active_revision_id = excluded.active_revision_id,
                 enabled = 1,
                 trust_level = excluded.trust_level,
                 install_status = 'active',
                 updated_at = excluded.updated_at"#,
        )
        .bind(package_id.as_str())
        .bind(layer.as_str())
        .bind(revision_id)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        insert_audit(
            &mut tx,
            actor_id,
            "activate_revision",
            package_id,
            Some(revision_id),
            "ok",
            Value::Object(Map::new()),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn create_approval(
        &self,
        input: NewSkillApproval,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let approval_id = Uuid::new_v4().to_string();
        let created_at = Utc::now();
        let permission_diff = serde_json::to_string(&input.permission_diff)?;
        sqlx::query(
            r#"INSERT INTO skill_approvals
               (approval_id, package_id, revision_id, operation, requested_by, approved_by,
                status, permission_diff, created_at, resolved_at)
               VALUES (?, ?, ?, ?, ?, NULL, 'pending', ?, ?, NULL)"#,
        )
        .bind(&approval_id)
        .bind(input.package_id.as_str())
        .bind(&input.revision_id)
        .bind(&input.operation)
        .bind(&input.requested_by)
        .bind(permission_diff)
        .bind(created_at.to_rfc3339())
        .execute(self.storage.pool())
        .await?;

        Ok(SkillApprovalRecord {
            approval_id,
            package_id: input.package_id,
            revision_id: input.revision_id,
            operation: input.operation,
            requested_by: input.requested_by,
            approved_by: None,
            status: SkillApprovalStatus::Pending,
            permission_diff: input.permission_diff,
            created_at,
            resolved_at: None,
        })
    }

    pub async fn get_approval(
        &self,
        approval_id: &str,
    ) -> anyhow::Result<Option<SkillApprovalRecord>> {
        let query = format!("SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?");
        sqlx::query(&query)
            .bind(approval_id)
            .fetch_optional(self.storage.pool())
            .await?
            .map(|row| approval_from_row(&row))
            .transpose()
    }

    pub async fn approve(
        &self,
        approval_id: &str,
        actor_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.resolve_approval(approval_id, actor_id, SkillApprovalStatus::Approved)
            .await
    }

    pub async fn reject(
        &self,
        approval_id: &str,
        actor_id: &str,
    ) -> anyhow::Result<SkillApprovalRecord> {
        self.resolve_approval(approval_id, actor_id, SkillApprovalStatus::Rejected)
            .await
    }

    async fn resolve_approval(
        &self,
        approval_id: &str,
        actor_id: &str,
        target: SkillApprovalStatus,
    ) -> anyhow::Result<SkillApprovalRecord> {
        let mut tx = self.storage.pool().begin().await?;
        let select =
            format!("SELECT {APPROVAL_COLUMNS} FROM skill_approvals WHERE approval_id = ?");
        let row = sqlx::query(&select)
            .bind(approval_id)
            .fetch_optional(&mut *tx)
            .await?
            .with_context(|| format!("skill approval not found: {approval_id}"))?;
        let approval = approval_from_row(&row)?;
        if approval.status != SkillApprovalStatus::Pending {
            anyhow::bail!("skill approval already resolved: {approval_id}");
        }
        if target == SkillApprovalStatus::Approved && approval.requested_by == actor_id {
            anyhow::bail!("requester cannot approve their own request");
        }

        let resolved_at = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE skill_approvals
               SET approved_by = ?, status = ?, resolved_at = ?
               WHERE approval_id = ? AND status = 'pending'"#,
        )
        .bind(actor_id)
        .bind(target.as_str())
        .bind(resolved_at)
        .bind(approval_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            anyhow::bail!("skill approval changed concurrently: {approval_id}");
        }
        let row = sqlx::query(&select)
            .bind(approval_id)
            .fetch_one(&mut *tx)
            .await?;
        let resolved = approval_from_row(&row)?;
        tx.commit().await?;
        Ok(resolved)
    }

    pub async fn list_audit(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Vec<SkillAuditRecord>> {
        let query = format!(
            "SELECT {AUDIT_COLUMNS} FROM skill_audit_log WHERE package_id = ? ORDER BY created_at, id"
        );
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_all(self.storage.pool())
            .await?
            .iter()
            .map(audit_from_row)
            .collect()
    }

    pub async fn mark_revision_quarantined(
        &self,
        revision_id: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        let now = Utc::now();
        let mut tx = self.storage.pool().begin().await?;
        let row = sqlx::query(
            "SELECT package_id, validation_json FROM skill_revisions WHERE revision_id = ?",
        )
        .bind(revision_id)
        .fetch_optional(&mut *tx)
        .await?
        .with_context(|| format!("skill revision not found: {revision_id}"))?;
        let package_id = SkillPackageId::parse(row.try_get("package_id")?)?;
        let validation_text: String = row.try_get("validation_json")?;
        let previous = parse_json("validation_json", &validation_text)?;
        let mut validation = match previous {
            Value::Object(map) => map,
            value => Map::from_iter([("previousValidation".into(), value)]),
        };
        validation.insert("quarantined".into(), Value::Bool(true));
        validation.insert("quarantineReason".into(), Value::String(reason.into()));
        validation.insert("quarantinedAt".into(), Value::String(now.to_rfc3339()));
        let validation = serde_json::to_string(&Value::Object(validation))?;
        let result =
            sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
                .bind(validation)
                .bind(revision_id)
                .execute(&mut *tx)
                .await?;
        ensure_changed(result.rows_affected(), "skill revision", revision_id)?;

        sqlx::query(
            r#"UPDATE skill_installations
               SET active_revision_id = NULL, enabled = 0, install_status = 'quarantined', updated_at = ?
               WHERE package_id = ? AND active_revision_id = ?"#,
        )
        .bind(now.to_rfc3339())
        .bind(package_id.as_str())
        .bind(revision_id)
        .execute(&mut *tx)
        .await?;
        insert_audit(
            &mut tx,
            "system",
            "mark_revision_quarantined",
            &package_id,
            Some(revision_id),
            "ok",
            serde_json::json!({"reason": reason}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn record_snapshot_candidate(
        &self,
        generation: u64,
        members: Value,
    ) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let members = serde_json::to_string(&members)?;
        sqlx::query(
            r#"INSERT INTO skill_snapshots
               (generation, status, members_json, created_at, activated_at)
               VALUES (?, 'candidate', ?, ?, NULL)"#,
        )
        .bind(generation)
        .bind(members)
        .bind(Utc::now().to_rfc3339())
        .execute(self.storage.pool())
        .await?;
        Ok(())
    }

    pub async fn mark_snapshot_active(&self, generation: u64) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let mut tx = self.storage.pool().begin().await?;
        ensure_snapshot_exists(&mut tx, generation).await?;
        sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE generation = ?")
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good'",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE skill_snapshots SET status = 'last_known_good' WHERE status = 'active'",
        )
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "UPDATE skill_snapshots SET status = 'active', activated_at = ? WHERE generation = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        ensure_changed(
            result.rows_affected(),
            "skill snapshot",
            &generation.to_string(),
        )?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn mark_snapshot_last_known_good(&self, generation: u64) -> anyhow::Result<()> {
        let generation = sqlite_generation(generation)?;
        let mut tx = self.storage.pool().begin().await?;
        ensure_snapshot_exists(&mut tx, generation).await?;
        sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE generation = ?")
            .bind(generation)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good'",
        )
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "UPDATE skill_snapshots SET status = 'last_known_good' WHERE generation = ?",
        )
        .bind(generation)
        .execute(&mut *tx)
        .await?;
        ensure_changed(
            result.rows_affected(),
            "skill snapshot",
            &generation.to_string(),
        )?;
        tx.commit().await?;
        Ok(())
    }
}

async fn ensure_snapshot_exists(
    tx: &mut Transaction<'_, Sqlite>,
    generation: i64,
) -> anyhow::Result<()> {
    let exists: i64 =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM skill_snapshots WHERE generation = ?)")
            .bind(generation)
            .fetch_one(&mut **tx)
            .await?;
    if exists == 0 {
        anyhow::bail!("skill snapshot not found: {generation}");
    }
    Ok(())
}

async fn insert_audit(
    tx: &mut Transaction<'_, Sqlite>,
    actor_id: &str,
    operation: &str,
    package_id: &SkillPackageId,
    revision_id: Option<&str>,
    result: &str,
    metadata: Value,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO skill_audit_log
           (id, actor_id, operation, package_id, revision_id, result, metadata_json, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .bind(actor_id)
    .bind(operation)
    .bind(package_id.as_str())
    .bind(revision_id)
    .bind(result)
    .bind(serde_json::to_string(&metadata)?)
    .bind(Utc::now().to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn revision_from_row(row: &SqliteRow) -> anyhow::Result<SkillRevisionRecord> {
    Ok(SkillRevisionRecord {
        revision_id: row.try_get("revision_id")?,
        package_id: parse_package_id(row, "package_id")?,
        version: row.try_get("version")?,
        content_hash: row.try_get("content_hash")?,
        storage_path: row.try_get("storage_path")?,
        descriptor_json: parse_row_json(row, "descriptor_json")?,
        validation_json: parse_row_json(row, "validation_json")?,
        created_by: row.try_get("created_by")?,
        created_at: parse_row_timestamp(row, "created_at")?,
    })
}

fn installation_from_row(row: &SqliteRow) -> anyhow::Result<SkillInstallationRecord> {
    let layer: String = row.try_get("source_layer")?;
    let status: String = row.try_get("install_status")?;
    let enabled: i64 = row.try_get("enabled")?;
    Ok(SkillInstallationRecord {
        package_id: parse_package_id(row, "package_id")?,
        source_layer: SkillLayerRecord::parse(&layer)?,
        active_revision_id: row.try_get("active_revision_id")?,
        enabled: parse_bool("enabled", enabled)?,
        trust_level: row.try_get("trust_level")?,
        status: SkillInstallStatus::parse(&status)?,
        installed_at: parse_row_timestamp(row, "installed_at")?,
        updated_at: parse_row_timestamp(row, "updated_at")?,
    })
}

fn approval_from_row(row: &SqliteRow) -> anyhow::Result<SkillApprovalRecord> {
    let status: String = row.try_get("status")?;
    Ok(SkillApprovalRecord {
        approval_id: row.try_get("approval_id")?,
        package_id: parse_package_id(row, "package_id")?,
        revision_id: row.try_get("revision_id")?,
        operation: row.try_get("operation")?,
        requested_by: row.try_get("requested_by")?,
        approved_by: row.try_get("approved_by")?,
        status: SkillApprovalStatus::parse(&status)?,
        permission_diff: parse_row_json(row, "permission_diff")?,
        created_at: parse_row_timestamp(row, "created_at")?,
        resolved_at: parse_optional_row_timestamp(row, "resolved_at")?,
    })
}

fn audit_from_row(row: &SqliteRow) -> anyhow::Result<SkillAuditRecord> {
    Ok(SkillAuditRecord {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        operation: row.try_get("operation")?,
        package_id: parse_package_id(row, "package_id")?,
        revision_id: row.try_get("revision_id")?,
        result: row.try_get("result")?,
        metadata_json: parse_row_json(row, "metadata_json")?,
        created_at: parse_row_timestamp(row, "created_at")?,
    })
}

fn parse_package_id(row: &SqliteRow, column: &str) -> anyhow::Result<SkillPackageId> {
    let value: String = row.try_get(column)?;
    SkillPackageId::parse(&value).with_context(|| format!("invalid {column}"))
}

fn parse_row_json(row: &SqliteRow, column: &str) -> anyhow::Result<Value> {
    let value: String = row.try_get(column)?;
    parse_json(column, &value)
}

fn parse_json(column: &str, value: &str) -> anyhow::Result<Value> {
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

fn sqlite_generation(generation: u64) -> anyhow::Result<i64> {
    i64::try_from(generation).context("snapshot generation exceeds SQLite INTEGER range")
}

fn ensure_changed(rows: u64, entity: &str, id: &str) -> anyhow::Result<()> {
    if rows == 0 {
        anyhow::bail!("{entity} not found: {id}");
    }
    Ok(())
}
