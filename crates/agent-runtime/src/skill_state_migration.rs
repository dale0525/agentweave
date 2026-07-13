use crate::skill_state_rows::{
    APPROVAL_COLUMNS, INSTALLATION_COLUMNS, REVISION_COLUMNS, approval_from_row,
    installation_from_row, revision_from_row, validate_installation_invariant,
};
use anyhow::Context;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use std::collections::HashSet;

const CREATE_REVISIONS: &str = r#"CREATE TABLE skill_revisions (
  revision_id TEXT PRIMARY KEY,
  package_id TEXT NOT NULL,
  version TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  storage_path TEXT NOT NULL,
  descriptor_json TEXT NOT NULL,
  validation_json TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  lifecycle_status TEXT NOT NULL CHECK(lifecycle_status IN ('staging', 'managed', 'quarantined')),
  UNIQUE(package_id, revision_id)
)"#;

const CREATE_INSTALLATIONS: &str = r#"CREATE TABLE skill_installations (
  package_id TEXT PRIMARY KEY,
  source_layer TEXT NOT NULL CHECK(source_layer IN ('builtin', 'managed', 'session')),
  active_revision_id TEXT,
  enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  trust_level TEXT NOT NULL CHECK(length(trust_level) > 0),
  install_status TEXT NOT NULL CHECK(install_status IN ('active', 'disabled', 'inactive', 'quarantined', 'removed')),
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK(install_status != 'active' OR (enabled = 1 AND active_revision_id IS NOT NULL)),
  FOREIGN KEY(package_id, active_revision_id) REFERENCES skill_revisions(package_id, revision_id)
)"#;

const CREATE_APPROVALS: &str = r#"CREATE TABLE skill_approvals (
  approval_id TEXT PRIMARY KEY,
  package_id TEXT NOT NULL,
  revision_id TEXT NOT NULL,
  operation TEXT NOT NULL,
  requested_by TEXT NOT NULL,
  approved_by TEXT,
  status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected')),
  permission_diff TEXT NOT NULL,
  created_at TEXT NOT NULL,
  resolved_at TEXT,
  CHECK(
    (status = 'pending' AND approved_by IS NULL AND resolved_at IS NULL)
    OR
    (status IN ('approved', 'rejected') AND approved_by IS NOT NULL AND resolved_at IS NOT NULL)
  )
)"#;

pub(crate) async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    let mut tx = crate::skill_state_transactions::begin_immediate(pool).await?;
    let result = async {
        ensure_supported_schema_versions(&mut tx).await?;
        create_migration_ledger(&mut tx).await?;
        migrate_approvals(&mut tx).await?;
        create_supporting_tables(&mut tx).await?;

        let revisions_exist = table_exists(&mut tx, "skill_revisions").await?;
        let installations_exist = table_exists(&mut tx, "skill_installations").await?;
        if !revisions_exist {
            sqlx::query(CREATE_REVISIONS).execute(&mut *tx).await?;
            if installations_exist {
                rebuild_installations(&mut tx).await?;
            } else {
                sqlx::query(CREATE_INSTALLATIONS).execute(&mut *tx).await?;
            }
        } else if !column_exists(&mut tx, "skill_revisions", "lifecycle_status").await? {
            upgrade_task6_schema(&mut tx, installations_exist).await?;
        } else if !installations_exist {
            sqlx::query(CREATE_INSTALLATIONS).execute(&mut *tx).await?;
        } else if !installation_schema_is_final(&mut tx).await? {
            rebuild_installations(&mut tx).await?;
        }

        create_indexes(&mut tx).await?;
        record_schema_version(&mut tx).await?;
        Ok(())
    }
    .await;
    crate::skill_state_transactions::finish(tx, result).await
}

const SKILL_SCHEMA_VERSION: i64 = 2;
const MIN_COMPATIBLE_SKILL_SCHEMA_VERSION: i64 = 1;

async fn ensure_supported_schema_versions(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    if !table_exists(tx, "skill_schema_migrations").await? {
        return Ok(());
    }
    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM skill_schema_migrations WHERE component = 'skill_state' ORDER BY version",
    )
    .fetch_all(&mut **tx)
    .await?;
    for version in versions {
        if version > SKILL_SCHEMA_VERSION {
            anyhow::bail!(
                "skill state schema version {version} is newer than supported version {SKILL_SCHEMA_VERSION}"
            );
        }
        anyhow::ensure!(
            version >= MIN_COMPATIBLE_SKILL_SCHEMA_VERSION,
            "skill state schema version {version} is outside the compatible range {MIN_COMPATIBLE_SKILL_SCHEMA_VERSION}..={SKILL_SCHEMA_VERSION}"
        );
    }
    Ok(())
}

async fn create_migration_ledger(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS skill_schema_migrations (
          component TEXT NOT NULL,
          version INTEGER NOT NULL CHECK(version > 0),
          applied_at TEXT NOT NULL,
          PRIMARY KEY(component, version)
        )"#,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_schema_version(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO skill_schema_migrations (component, version, applied_at)
           VALUES ('skill_state', ?, ?)
           ON CONFLICT(component, version) DO NOTHING"#,
    )
    .bind(SKILL_SCHEMA_VERSION)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn schema_version_is_current(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<bool> {
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(version) FROM skill_schema_migrations WHERE component = 'skill_state'",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(version.is_some_and(|version| version >= SKILL_SCHEMA_VERSION))
}

async fn create_supporting_tables(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    for statement in [
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
        r#"CREATE TABLE IF NOT EXISTS skill_approval_bindings (
          approval_id TEXT PRIMARY KEY,
          binding_json TEXT NOT NULL,
          FOREIGN KEY(approval_id) REFERENCES skill_approvals(approval_id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_circuit_state (
          revision_id TEXT PRIMARY KEY,
          consecutive_failures INTEGER NOT NULL CHECK(consecutive_failures >= 0),
          open_until TEXT,
          updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_circuit_omissions (
          revision_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          omitted_generation INTEGER NOT NULL CHECK(omitted_generation >= 0),
          consumed_generation INTEGER CHECK(consumed_generation >= omitted_generation),
          created_at TEXT NOT NULL,
          consumed_at TEXT,
          CHECK(
            (consumed_generation IS NULL AND consumed_at IS NULL)
            OR (consumed_generation IS NOT NULL AND consumed_at IS NOT NULL)
          ),
          FOREIGN KEY(revision_id) REFERENCES skill_revisions(revision_id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_revision_retention (
          revision_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          reason TEXT NOT NULL,
          retain_until TEXT NOT NULL,
          created_at TEXT NOT NULL,
          FOREIGN KEY(revision_id) REFERENCES skill_revisions(revision_id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_revision_cleanup (
          revision_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          expected_json TEXT NOT NULL,
          status TEXT NOT NULL CHECK(status = 'pending'),
          created_at TEXT NOT NULL,
          FOREIGN KEY(revision_id) REFERENCES skill_revisions(revision_id) ON DELETE CASCADE
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_maintenance_diagnostics (
          idempotency_key TEXT PRIMARY KEY,
          revision_id TEXT,
          area TEXT NOT NULL,
          operation TEXT NOT NULL,
          outcome TEXT NOT NULL,
          metadata_json TEXT NOT NULL,
          created_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_application_state (
          singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
          graph_fingerprint TEXT NOT NULL CHECK(length(graph_fingerprint) = 64),
          snapshot_generation INTEGER NOT NULL CHECK(snapshot_generation >= 0),
          updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_snapshot_leases (
          lease_id TEXT PRIMARY KEY,
          generation INTEGER NOT NULL CHECK(generation >= 0),
          members_json TEXT NOT NULL,
          expires_at TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE IF NOT EXISTS skill_snapshot_lease_revisions (
          lease_id TEXT NOT NULL,
          revision_id TEXT NOT NULL,
          PRIMARY KEY(lease_id, revision_id),
          FOREIGN KEY(lease_id) REFERENCES skill_snapshot_leases(lease_id) ON DELETE CASCADE
        )"#,
    ] {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn migrate_approvals(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    if !table_exists(tx, "skill_approvals").await? {
        sqlx::query(CREATE_APPROVALS).execute(&mut **tx).await?;
        return Ok(());
    }
    if approval_schema_is_final(tx).await? {
        return Ok(());
    }

    sqlx::query("ALTER TABLE skill_approvals RENAME TO skill_approvals_legacy")
        .execute(&mut **tx)
        .await?;
    sqlx::query(CREATE_APPROVALS).execute(&mut **tx).await?;
    validate_approval_rows(tx, "skill_approvals_legacy").await?;
    copy_approvals(tx, "skill_approvals_legacy").await?;
    sqlx::query("DROP TABLE skill_approvals_legacy")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn validate_approval_rows(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> anyhow::Result<()> {
    let statement = format!("SELECT {APPROVAL_COLUMNS} FROM {table} ORDER BY approval_id");
    let rows = sqlx::query(&statement).fetch_all(&mut **tx).await?;
    for row in rows {
        let identity: String = row
            .try_get("approval_id")
            .unwrap_or_else(|_| "<unreadable>".into());
        approval_from_row(&row).with_context(|| format!("skill_approvals row {identity}"))?;
    }
    Ok(())
}

async fn copy_approvals(
    tx: &mut Transaction<'_, Sqlite>,
    source_table: &str,
) -> anyhow::Result<()> {
    let statement = format!(
        r#"INSERT INTO skill_approvals
           (approval_id, package_id, revision_id, operation, requested_by, approved_by,
            status, permission_diff, created_at, resolved_at)
           SELECT approval_id, package_id, revision_id, operation, requested_by, approved_by,
                  status, permission_diff, created_at, resolved_at
           FROM {source_table}"#
    );
    sqlx::query(&statement).execute(&mut **tx).await?;
    Ok(())
}

async fn upgrade_task6_schema(
    tx: &mut Transaction<'_, Sqlite>,
    installations_exist: bool,
) -> anyhow::Result<()> {
    if installations_exist {
        sqlx::query("ALTER TABLE skill_installations RENAME TO skill_installations_task6_legacy")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("ALTER TABLE skill_revisions RENAME TO skill_revisions_task6_legacy")
        .execute(&mut **tx)
        .await?;
    sqlx::query(CREATE_REVISIONS).execute(&mut **tx).await?;
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at, lifecycle_status)
           SELECT revision_id, package_id, version, content_hash, storage_path, descriptor_json,
                  validation_json, created_by, created_at, 'managed'
           FROM skill_revisions_task6_legacy"#,
    )
    .execute(&mut **tx)
    .await?;
    let revision_pairs = validate_revision_rows(tx, "skill_revisions").await?;
    sqlx::query(CREATE_INSTALLATIONS).execute(&mut **tx).await?;
    if installations_exist {
        validate_installation_rows(tx, "skill_installations_task6_legacy", &revision_pairs).await?;
        copy_installations(tx, "skill_installations_task6_legacy").await?;
        sqlx::query("DROP TABLE skill_installations_task6_legacy")
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("DROP TABLE skill_revisions_task6_legacy")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn rebuild_installations(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    sqlx::query("ALTER TABLE skill_installations RENAME TO skill_installations_legacy")
        .execute(&mut **tx)
        .await?;
    sqlx::query(CREATE_INSTALLATIONS).execute(&mut **tx).await?;
    let revision_pairs = validate_revision_rows(tx, "skill_revisions").await?;
    validate_installation_rows(tx, "skill_installations_legacy", &revision_pairs).await?;
    copy_installations(tx, "skill_installations_legacy").await?;
    sqlx::query("DROP TABLE skill_installations_legacy")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn copy_installations(
    tx: &mut Transaction<'_, Sqlite>,
    source_table: &str,
) -> anyhow::Result<()> {
    let statement = format!(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           SELECT package_id, source_layer, active_revision_id, enabled, trust_level,
                  install_status, installed_at, updated_at
           FROM {source_table}"#
    );
    sqlx::query(&statement).execute(&mut **tx).await?;
    Ok(())
}

async fn validate_revision_rows(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> anyhow::Result<HashSet<(String, String)>> {
    let statement = format!("SELECT {REVISION_COLUMNS} FROM {table} ORDER BY revision_id");
    let rows = sqlx::query(&statement).fetch_all(&mut **tx).await?;
    let mut pairs = HashSet::with_capacity(rows.len());
    for row in rows {
        let identity: String = row
            .try_get("revision_id")
            .unwrap_or_else(|_| "<unreadable>".into());
        let revision =
            revision_from_row(&row).with_context(|| format!("skill_revisions row {identity}"))?;
        pairs.insert((
            revision.package_id.as_str().to_string(),
            revision.revision_id,
        ));
    }
    Ok(pairs)
}

async fn validate_installation_rows(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    revision_pairs: &HashSet<(String, String)>,
) -> anyhow::Result<()> {
    let statement = format!("SELECT {INSTALLATION_COLUMNS} FROM {table} ORDER BY package_id");
    let rows = sqlx::query(&statement).fetch_all(&mut **tx).await?;
    for row in rows {
        let identity: String = row
            .try_get("package_id")
            .unwrap_or_else(|_| "<unreadable>".into());
        let installation = installation_from_row(&row)
            .with_context(|| format!("skill_installations row {identity}"))?;
        validate_installation_invariant(
            installation.status,
            installation.enabled,
            installation.active_revision_id.as_deref(),
        )
        .with_context(|| format!("skill_installations row {identity}"))?;
        if let Some(revision_id) = &installation.active_revision_id {
            let pair = (
                installation.package_id.as_str().to_string(),
                revision_id.clone(),
            );
            if !revision_pairs.contains(&pair) {
                anyhow::bail!(
                    "skill_installations row {identity}: revision {revision_id} does not belong to package"
                );
            }
        }
    }
    Ok(())
}

async fn create_indexes(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<()> {
    for statement in [
        "CREATE INDEX IF NOT EXISTS idx_skill_revisions_package ON skill_revisions(package_id, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_skill_installations_active ON skill_installations(enabled, install_status)",
        "CREATE INDEX IF NOT EXISTS idx_skill_approvals_package_status ON skill_approvals(package_id, status, created_at)",
        "CREATE INDEX IF NOT EXISTS idx_skill_audit_package_created ON skill_audit_log(package_id, created_at, id)",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_snapshots_single_active ON skill_snapshots(status) WHERE status = 'active'",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_snapshots_single_lkg ON skill_snapshots(status) WHERE status = 'last_known_good'",
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_skill_approvals_single_removal ON skill_approvals(package_id, operation) WHERE status = 'pending' AND operation = 'remove'",
        "CREATE INDEX IF NOT EXISTS idx_skill_snapshot_leases_expiry ON skill_snapshot_leases(expires_at, generation)",
        "CREATE INDEX IF NOT EXISTS idx_skill_snapshot_lease_revisions_revision ON skill_snapshot_lease_revisions(revision_id, lease_id)",
    ] {
        sqlx::query(statement).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn table_exists(tx: &mut Transaction<'_, Sqlite>, table_name: &str) -> anyhow::Result<bool> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
    )
    .bind(table_name)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists != 0)
}

async fn column_exists(
    tx: &mut Transaction<'_, Sqlite>,
    table_name: &str,
    column_name: &str,
) -> anyhow::Result<bool> {
    let statement = format!("PRAGMA table_info({table_name})");
    let rows = sqlx::query(&statement).fetch_all(&mut **tx).await?;
    for row in rows {
        let name: String = row.try_get("name")?;
        if name == column_name {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn installation_schema_is_final(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<bool> {
    let expected_columns = [
        "package_id",
        "source_layer",
        "active_revision_id",
        "enabled",
        "trust_level",
        "install_status",
        "installed_at",
        "updated_at",
    ];
    if table_columns(tx, "skill_installations").await? != expected_columns {
        return Ok(false);
    }
    let foreign_keys: Vec<(String, String, String)> = sqlx::query_as(
        r#"SELECT "table", "from", "to"
           FROM pragma_foreign_key_list('skill_installations') ORDER BY seq"#,
    )
    .fetch_all(&mut **tx)
    .await?;
    let expected_foreign_keys = vec![
        (
            "skill_revisions".to_string(),
            "package_id".to_string(),
            "package_id".to_string(),
        ),
        (
            "skill_revisions".to_string(),
            "active_revision_id".to_string(),
            "revision_id".to_string(),
        ),
    ];
    if foreign_keys != expected_foreign_keys
        || !has_unique_index(tx, "skill_revisions", &["package_id", "revision_id"]).await?
    {
        return Ok(false);
    }
    if schema_version_is_current(tx).await? {
        return Ok(true);
    }
    constraints_reject_all(
        tx,
        &[
            installation_constraint_probe("source", "invalid", 0, "approved", "disabled", false),
            installation_constraint_probe("enabled", "managed", 2, "approved", "disabled", false),
            installation_constraint_probe("trust", "managed", 0, "", "disabled", false),
            installation_constraint_probe("status", "managed", 0, "approved", "invalid", false),
            installation_constraint_probe("active", "managed", 1, "approved", "active", false),
        ],
    )
    .await
}

async fn approval_schema_is_final(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<bool> {
    let expected_columns = [
        "approval_id",
        "package_id",
        "revision_id",
        "operation",
        "requested_by",
        "approved_by",
        "status",
        "permission_diff",
        "created_at",
        "resolved_at",
    ];
    if table_columns(tx, "skill_approvals").await? != expected_columns {
        return Ok(false);
    }
    if schema_version_is_current(tx).await? {
        return Ok(true);
    }
    constraint_rejects(
        tx,
        r#"INSERT INTO skill_approvals
           (approval_id, package_id, revision_id, operation, requested_by, approved_by,
            status, permission_diff, created_at, resolved_at)
           VALUES ('migration-probe-approval', 'migration.probe.approval', 'revision',
                   'activate', 'requester', 'approver', 'pending', '{}',
                   '2026-01-01T00:00:00Z', NULL)"#,
        "DELETE FROM skill_approvals WHERE approval_id = 'migration-probe-approval'",
    )
    .await
}

async fn table_columns(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query("SELECT name FROM pragma_table_info(?) ORDER BY cid")
        .bind(table)
        .fetch_all(&mut **tx)
        .await?;
    rows.iter()
        .map(|row| row.try_get("name").map_err(Into::into))
        .collect()
}

async fn has_unique_index(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    expected: &[&str],
) -> anyhow::Result<bool> {
    let indexes = sqlx::query("SELECT name, \"unique\" FROM pragma_index_list(?)")
        .bind(table)
        .fetch_all(&mut **tx)
        .await?;
    for index in indexes {
        let unique: i64 = index.try_get("unique")?;
        if unique == 0 {
            continue;
        }
        let name: String = index.try_get("name")?;
        let columns = sqlx::query(
            "SELECT name FROM pragma_index_xinfo(?) WHERE key = 1 AND cid >= 0 ORDER BY seqno",
        )
        .bind(name)
        .fetch_all(&mut **tx)
        .await?
        .iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()?;
        if columns
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn constraint_rejects(
    tx: &mut Transaction<'_, Sqlite>,
    invalid_insert: &str,
    cleanup: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(invalid_insert).execute(&mut **tx).await;
    if result.is_err() {
        return Ok(true);
    }
    sqlx::query(cleanup).execute(&mut **tx).await?;
    Ok(false)
}

async fn constraints_reject_all(
    tx: &mut Transaction<'_, Sqlite>,
    probes: &[(String, String)],
) -> anyhow::Result<bool> {
    for (insert, cleanup) in probes {
        if !constraint_rejects(tx, insert, cleanup).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn installation_constraint_probe(
    suffix: &str,
    source_layer: &str,
    enabled: i64,
    trust_level: &str,
    install_status: &str,
    with_revision: bool,
) -> (String, String) {
    let package_id = format!("migration.probe.installation.{suffix}");
    let revision = if with_revision { "'revision'" } else { "NULL" };
    (
        format!(
            r#"INSERT INTO skill_installations
               (package_id, source_layer, active_revision_id, enabled, trust_level,
                install_status, installed_at, updated_at)
               VALUES ('{package_id}', '{source_layer}', {revision}, {enabled}, '{trust_level}',
                       '{install_status}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#
        ),
        format!("DELETE FROM skill_installations WHERE package_id = '{package_id}'"),
    )
}
