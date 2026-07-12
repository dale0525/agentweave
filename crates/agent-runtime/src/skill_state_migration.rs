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
        Ok(())
    }
    .await;
    crate::skill_state_transactions::finish(tx, result).await
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
    let sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'skill_installations'",
    )
    .fetch_one(&mut **tx)
    .await?;
    let foreign_keys: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('skill_installations') WHERE \"table\" = 'skill_revisions'",
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(sql.contains("active_revision_id IS NOT NULL") && foreign_keys == 2)
}

async fn approval_schema_is_final(tx: &mut Transaction<'_, Sqlite>) -> anyhow::Result<bool> {
    let sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'skill_approvals'",
    )
    .fetch_one(&mut **tx)
    .await?;
    let sql = sql.to_ascii_lowercase();
    Ok(sql.contains("status = 'pending'")
        && sql.contains("approved_by is null")
        && sql.contains("resolved_at is null")
        && sql.contains("approved_by is not null")
        && sql.contains("resolved_at is not null"))
}
