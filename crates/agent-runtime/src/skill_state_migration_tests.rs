use crate::skill_state::{SkillApprovalStatus, SkillRevisionStatus, SkillStateStore};
use crate::storage::Storage;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Barrier, mpsc};
use tokio::time::timeout;
use uuid::Uuid;

fn file_database() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("skill-state.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    (directory, url)
}

async fn raw_pool(url: &str) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(url)
        .unwrap()
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap()
}

async fn await_operation_entries(receiver: &mut mpsc::UnboundedReceiver<()>) {
    for _ in 0..2 {
        timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("connect task did not reach the operation entry")
            .expect("operation entry channel closed");
    }
}

async fn await_task<T>(task: tokio::task::JoinHandle<T>) -> T {
    timeout(Duration::from_secs(3), task)
        .await
        .expect("connect task did not finish")
        .expect("connect task panicked")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_storage_connect_serializes_skill_migration_without_sqlite_busy() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_core_storage_tables(&pool).await;
    create_legacy_task6_tables(&pool).await;
    create_legacy_approval_table(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    let approval_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    insert_legacy_installation(
        &pool,
        "com.example.calendar",
        Some(&revision_id),
        1,
        "active",
    )
    .await;
    insert_legacy_approval(&pool, &approval_id, "pending", None, None).await;

    let mut lock = pool.acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (entered, mut entries) = mpsc::unbounded_channel();
    let first_barrier = barrier.clone();
    let first_entered = entered.clone();
    let first_url = url.clone();
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        first_entered.send(()).unwrap();
        Storage::connect(&first_url).await
    });
    let second_barrier = barrier.clone();
    let second_entered = entered.clone();
    let second_url = url.clone();
    let second = tokio::spawn(async move {
        second_barrier.wait().await;
        second_entered.send(()).unwrap();
        Storage::connect(&second_url).await
    });
    drop(entered);
    barrier.wait().await;
    await_operation_entries(&mut entries).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(!first.is_finished());
    assert!(!second.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);
    pool.close().await;

    let first = await_task(first).await;
    let second = await_task(second).await;
    let storage = first.unwrap_or_else(|error| panic!("first connect failed: {error:#}"));
    let _second = second.unwrap_or_else(|error| panic!("second connect failed: {error:#}"));
    let state = SkillStateStore::new(storage.clone());
    assert_eq!(
        state
            .get_revision(&revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Managed
    );
    assert!(
        state
            .get_installation(
                &crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap()
            )
            .await
            .unwrap()
            .unwrap()
            .enabled
    );
    assert_eq!(
        state
            .get_approval(&approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Pending
    );
    let legacy_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%legacy%'",
    )
    .fetch_one(storage.pool())
    .await
    .unwrap();
    assert_eq!(legacy_tables, 0);
    let foreign_key_errors = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(storage.pool())
        .await
        .unwrap();
    assert!(foreign_key_errors.is_empty());
}

#[tokio::test]
async fn upgrades_task6_schema_with_lifecycle_composite_fk_and_active_check() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    insert_legacy_installation(
        &pool,
        "com.example.calendar",
        Some(&revision_id),
        1,
        "active",
    )
    .await;
    pool.close().await;

    let storage = Storage::connect(&url).await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let revision = state.get_revision(&revision_id).await.unwrap().unwrap();
    assert_eq!(revision.status, SkillRevisionStatus::Managed);

    let foreign_keys = sqlx::query("PRAGMA foreign_key_list(skill_installations)")
        .fetch_all(storage.pool())
        .await
        .unwrap();
    assert_eq!(foreign_keys.len(), 2);
    let active_without_revision = sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.missing', 'managed', NULL, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(storage.pool())
    .await;
    assert!(active_without_revision.is_err());
    let cross_package_revision = sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.mail', 'managed', ?, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind(&revision_id)
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(storage.pool())
    .await;
    assert!(cross_package_revision.is_err());

    crate::skill_state::migrate(storage.pool()).await.unwrap();
    crate::skill_state::migrate(storage.pool()).await.unwrap();
}

#[tokio::test]
async fn upgrades_legacy_approval_schema_and_preserves_valid_rows() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_approval_table(&pool).await;
    let pending_id = Uuid::new_v4().to_string();
    let approved_id = Uuid::new_v4().to_string();
    insert_legacy_approval(&pool, &pending_id, "pending", None, None).await;
    insert_legacy_approval(
        &pool,
        &approved_id,
        "approved",
        Some("owner-2"),
        Some("2026-01-01T00:00:00Z"),
    )
    .await;
    pool.close().await;

    let storage = Storage::connect(&url).await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    assert_eq!(
        state
            .get_approval(&pending_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Pending
    );
    assert_eq!(
        state
            .get_approval(&approved_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Approved
    );
    let invalid_pending = sqlx::query(
        r#"INSERT INTO skill_approvals
           (approval_id, package_id, revision_id, operation, requested_by, approved_by,
            status, permission_diff, created_at, resolved_at)
           VALUES (?, 'com.example.calendar', 'rev-1', 'activate', 'owner-1', 'owner-2',
                   'pending', '[]', '2026-01-01T00:00:00Z', NULL)"#,
    )
    .bind(Uuid::new_v4().to_string())
    .execute(storage.pool())
    .await;
    assert!(invalid_pending.is_err());
    crate::skill_state::migrate(storage.pool()).await.unwrap();
}

#[tokio::test]
async fn structurally_final_quoted_schema_is_not_rebuilt_and_records_schema_version() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_core_storage_tables(&pool).await;
    create_structurally_final_quoted_skill_tables(&pool).await;
    pool.close().await;

    let storage = Storage::connect(&url).await.unwrap();
    for index in ["preserve_installation_marker", "preserve_approval_marker"] {
        let present: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
        )
        .bind(index)
        .fetch_one(storage.pool())
        .await
        .unwrap();
        assert_eq!(present, 1, "semantic schema was unnecessarily rebuilt");
    }
    let version: i64 = sqlx::query_scalar(
        "SELECT MAX(version) FROM skill_schema_migrations WHERE component = 'skill_state'",
    )
    .fetch_one(storage.pool())
    .await
    .unwrap();
    assert!(version >= 1);
}

#[tokio::test]
async fn future_schema_version_is_rejected_before_any_skill_schema_write() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    sqlx::query(
        r#"CREATE TABLE skill_schema_migrations (
          component TEXT NOT NULL,
          version INTEGER NOT NULL CHECK(version > 0),
          applied_at TEXT NOT NULL,
          PRIMARY KEY(component, version)
        )"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO skill_schema_migrations (component, version, applied_at) VALUES ('skill_state', 3, '2030-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TABLE future_skill_state_marker (value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO future_skill_state_marker (value) VALUES ('unchanged')")
        .execute(&pool)
        .await
        .unwrap();
    let schema_before = user_schema(&pool).await;
    let ledger_before: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT component, version, applied_at FROM skill_schema_migrations ORDER BY component, version",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let error = crate::skill_state::migrate(&pool)
        .await
        .expect_err("future skill schema must fail closed");

    assert!(error.to_string().contains("newer"), "{error:#}");
    assert_eq!(user_schema(&pool).await, schema_before);
    let ledger_after: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT component, version, applied_at FROM skill_schema_migrations ORDER BY component, version",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(ledger_after, ledger_before);
    let marker: String = sqlx::query_scalar("SELECT value FROM future_skill_state_marker")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(marker, "unchanged");
}

#[tokio::test]
async fn structurally_similar_schema_with_weakened_checks_is_rebuilt() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_core_storage_tables(&pool).await;
    create_weakened_check_skill_tables(&pool).await;
    pool.close().await;

    let storage = Storage::connect(&url).await.unwrap();
    for (index, expected) in [("weak_installation_marker", 0), ("weak_approval_marker", 1)] {
        let present: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
        )
        .bind(index)
        .fetch_one(storage.pool())
        .await
        .unwrap();
        assert_eq!(
            present, expected,
            "schema rebuild decision was not semantic"
        );
    }
    let invalid_enabled = sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('migration.invalid.enabled', 'managed', NULL, 2, 'approved',
                   'disabled', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')"#,
    )
    .execute(storage.pool())
    .await;
    assert!(invalid_enabled.is_err());
}

#[tokio::test]
async fn legacy_approval_migration_rejects_pending_with_resolver_and_rolls_back() {
    assert_legacy_approval_rejected("pending", Some("owner-2"), None, "pending").await;
}

#[tokio::test]
async fn legacy_approval_migration_rejects_resolved_with_null_resolver_and_rolls_back() {
    assert_legacy_approval_rejected("approved", None, Some("2026-01-01T00:00:00Z"), "resolved")
        .await;
}

#[tokio::test]
async fn legacy_approval_migration_rejects_bad_resolved_at_and_rolls_back() {
    assert_legacy_approval_rejected(
        "rejected",
        Some("owner-2"),
        Some("not-a-time"),
        "resolved_at",
    )
    .await;
}

#[tokio::test]
async fn failed_legacy_schema_upgrade_rolls_back_all_skill_ddl() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    insert_legacy_installation(&pool, "com.example.calendar", None, 1, "active").await;
    create_core_storage_tables(&pool).await;
    let schema_before = user_schema(&pool).await;
    pool.close().await;

    assert!(Storage::connect(&url).await.is_err());
    let pool = raw_pool(&url).await;
    assert_eq!(user_schema(&pool).await, schema_before);
    pool.close().await;
    assert_legacy_tables_intact(&url, 1, 1).await;
}

#[tokio::test]
async fn legacy_migration_rejects_empty_revision_storage_path_and_rolls_back() {
    assert_legacy_storage_path_rejected("").await;
}

#[tokio::test]
async fn legacy_migration_rejects_whitespace_revision_storage_path_and_rolls_back() {
    assert_legacy_storage_path_rejected("   ").await;
}

#[tokio::test]
async fn legacy_migration_rejects_bad_revision_uuid_with_identity_and_rolls_back() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    insert_legacy_revision(
        &pool,
        "bad-revision",
        "com.example.calendar",
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains("skill_revisions row bad-revision"),
        "{error}"
    );
    assert!(error.contains("UUID v4"), "{error}");
    assert_legacy_tables_intact(&url, 1, 0).await;
}

#[tokio::test]
async fn legacy_migration_rejects_bad_revision_json_with_identity_and_rolls_back() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{bad",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains(&format!("skill_revisions row {revision_id}")),
        "{error}"
    );
    assert!(error.contains("descriptor_json"), "{error}");
    assert_legacy_tables_intact(&url, 1, 0).await;
}

#[tokio::test]
async fn legacy_migration_rejects_bad_revision_timestamp_with_identity_and_rolls_back() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{}",
        "{}",
        "not-a-time",
    )
    .await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains(&format!("skill_revisions row {revision_id}")),
        "{error}"
    );
    assert!(error.contains("created_at"), "{error}");
    assert_legacy_tables_intact(&url, 1, 0).await;
}

#[tokio::test]
async fn legacy_migration_rejects_invalid_active_installation_with_identity_and_rolls_back() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(
        &pool,
        &revision_id,
        "com.example.calendar",
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    insert_legacy_installation(
        &pool,
        "com.example.calendar",
        Some(&revision_id),
        0,
        "active",
    )
    .await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains("skill_installations row com.example.calendar"),
        "{error}"
    );
    assert!(error.contains("active installation"), "{error}");
    assert_legacy_tables_intact(&url, 1, 1).await;
}

async fn storage_connect_error(url: &str) -> String {
    match Storage::connect(url).await {
        Ok(_) => panic!("legacy migration unexpectedly succeeded"),
        Err(error) => format!("{error:#}"),
    }
}

async fn assert_legacy_storage_path_rejected(storage_path: &str) {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision_with_path(
        &pool,
        &revision_id,
        "com.example.calendar",
        storage_path,
        "{}",
        "{}",
        "2026-01-01T00:00:00Z",
    )
    .await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains(&format!("skill_revisions row {revision_id}")),
        "{error}"
    );
    assert!(error.contains("storage path"), "{error}");
    assert_legacy_tables_intact(&url, 1, 0).await;
}

async fn assert_legacy_approval_rejected(
    status: &str,
    approved_by: Option<&str>,
    resolved_at: Option<&str>,
    expected_error: &str,
) {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_core_storage_tables(&pool).await;
    create_legacy_approval_table(&pool).await;
    let approval_id = Uuid::new_v4().to_string();
    insert_legacy_approval(&pool, &approval_id, status, approved_by, resolved_at).await;
    let schema_before = user_schema(&pool).await;
    pool.close().await;

    let error = storage_connect_error(&url).await;
    assert!(
        error.contains(&format!("skill_approvals row {approval_id}")),
        "{error}"
    );
    assert!(error.contains(expected_error), "{error}");
    let pool = raw_pool(&url).await;
    assert_eq!(user_schema(&pool).await, schema_before);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_approvals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

async fn user_schema(pool: &SqlitePool) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query_as(
        r#"SELECT type, name, tbl_name, sql
           FROM sqlite_master
           WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%'
           ORDER BY type, name"#,
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn assert_legacy_tables_intact(url: &str, revisions: i64, installations: i64) {
    let pool = raw_pool(url).await;
    let lifecycle_column: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('skill_revisions') WHERE name = 'lifecycle_status'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lifecycle_column, 0);
    let revision_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let installation_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_installations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(revision_count, revisions);
    assert_eq!(installation_count, installations);
    let renamed_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE '%legacy%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(renamed_tables, 0);
}

async fn create_legacy_task6_tables(pool: &SqlitePool) {
    sqlx::query(
        r#"CREATE TABLE skill_revisions (
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
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE TABLE skill_installations (
          package_id TEXT PRIMARY KEY,
          source_layer TEXT NOT NULL CHECK(source_layer IN ('builtin', 'managed', 'session')),
          active_revision_id TEXT,
          enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
          trust_level TEXT NOT NULL CHECK(length(trust_level) > 0),
          install_status TEXT NOT NULL CHECK(install_status IN ('active', 'disabled', 'inactive', 'quarantined', 'removed')),
          installed_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX idx_skill_revisions_package ON skill_revisions(package_id, created_at)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX idx_skill_installations_active ON skill_installations(enabled, install_status)",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn create_structurally_final_quoted_skill_tables(pool: &SqlitePool) {
    for statement in [
        r#"CREATE TABLE "skill_revisions" (
          "revision_id" TEXT PRIMARY KEY, "package_id" TEXT NOT NULL,
          "version" TEXT NOT NULL, "content_hash" TEXT NOT NULL,
          "storage_path" TEXT NOT NULL, "descriptor_json" TEXT NOT NULL,
          "validation_json" TEXT NOT NULL, "created_by" TEXT NOT NULL,
          "created_at" TEXT NOT NULL, "lifecycle_status" TEXT NOT NULL,
          UNIQUE("package_id", "revision_id")
        )"#,
        r#"CREATE TABLE "skill_installations" (
          "package_id" TEXT PRIMARY KEY,
          "source_layer" TEXT NOT NULL CHECK ("source_layer" IN ('builtin','managed','session')),
          "active_revision_id" TEXT,
          "enabled" INTEGER NOT NULL CHECK ("enabled" IN (0,1)),
          "trust_level" TEXT NOT NULL CHECK (length("trust_level") > 0),
          "install_status" TEXT NOT NULL CHECK ("install_status" IN ('active','disabled','inactive','quarantined','removed')),
          "installed_at" TEXT NOT NULL, "updated_at" TEXT NOT NULL,
          CHECK ( "install_status" <> 'active' OR ("enabled" = 1 AND "active_revision_id" IS NOT NULL) ),
          FOREIGN KEY("package_id", "active_revision_id")
            REFERENCES "skill_revisions"("package_id", "revision_id")
        )"#,
        r#"CREATE TABLE "skill_approvals" (
          "approval_id" TEXT PRIMARY KEY, "package_id" TEXT NOT NULL,
          "revision_id" TEXT NOT NULL, "operation" TEXT NOT NULL,
          "requested_by" TEXT NOT NULL, "approved_by" TEXT,
          "status" TEXT NOT NULL, "permission_diff" TEXT NOT NULL,
          "created_at" TEXT NOT NULL, "resolved_at" TEXT,
          CHECK (("status" = 'pending' AND "approved_by" IS NULL AND "resolved_at" IS NULL)
            OR ("status" IN ('approved','rejected') AND "approved_by" IS NOT NULL AND "resolved_at" IS NOT NULL))
        )"#,
        "CREATE INDEX preserve_installation_marker ON skill_installations(updated_at)",
        "CREATE INDEX preserve_approval_marker ON skill_approvals(created_at)",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
}

async fn create_weakened_check_skill_tables(pool: &SqlitePool) {
    for statement in [
        r#"CREATE TABLE skill_revisions (
          revision_id TEXT PRIMARY KEY, package_id TEXT NOT NULL, version TEXT NOT NULL,
          content_hash TEXT NOT NULL, storage_path TEXT NOT NULL, descriptor_json TEXT NOT NULL,
          validation_json TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL,
          lifecycle_status TEXT NOT NULL, UNIQUE(package_id, revision_id)
        )"#,
        r#"CREATE TABLE skill_installations (
          package_id TEXT PRIMARY KEY, source_layer TEXT NOT NULL, active_revision_id TEXT,
          enabled INTEGER NOT NULL, trust_level TEXT NOT NULL, install_status TEXT NOT NULL,
          installed_at TEXT NOT NULL, updated_at TEXT NOT NULL,
          CHECK(install_status != 'active' OR (enabled = 1 AND active_revision_id IS NOT NULL)),
          FOREIGN KEY(package_id, active_revision_id)
            REFERENCES skill_revisions(package_id, revision_id)
        )"#,
        r#"CREATE TABLE skill_approvals (
          approval_id TEXT PRIMARY KEY, package_id TEXT NOT NULL, revision_id TEXT NOT NULL,
          operation TEXT NOT NULL, requested_by TEXT NOT NULL, approved_by TEXT,
          status TEXT NOT NULL, permission_diff TEXT NOT NULL, created_at TEXT NOT NULL,
          resolved_at TEXT,
          CHECK((status = 'pending' AND approved_by IS NULL AND resolved_at IS NULL)
            OR (status IN ('approved','rejected') AND approved_by IS NOT NULL AND resolved_at IS NOT NULL))
        )"#,
        "CREATE INDEX weak_installation_marker ON skill_installations(updated_at)",
        "CREATE INDEX weak_approval_marker ON skill_approvals(created_at)",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
}

async fn create_core_storage_tables(pool: &SqlitePool) {
    for statement in [
        r#"CREATE TABLE sessions (
          id TEXT PRIMARY KEY,
          title TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        )"#,
        r#"CREATE TABLE runtime_settings (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        )"#,
        r#"CREATE TABLE messages (
          id TEXT PRIMARY KEY,
          session_id TEXT NOT NULL,
          role TEXT NOT NULL,
          content TEXT NOT NULL,
          created_at TEXT NOT NULL,
          FOREIGN KEY(session_id) REFERENCES sessions(id)
        )"#,
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
}

async fn create_legacy_approval_table(pool: &SqlitePool) {
    sqlx::query(
        r#"CREATE TABLE skill_approvals (
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
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX idx_skill_approvals_package_status ON skill_approvals(package_id, status, created_at)",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_legacy_approval(
    pool: &SqlitePool,
    approval_id: &str,
    status: &str,
    approved_by: Option<&str>,
    resolved_at: Option<&str>,
) {
    sqlx::query(
        r#"INSERT INTO skill_approvals
           (approval_id, package_id, revision_id, operation, requested_by, approved_by,
            status, permission_diff, created_at, resolved_at)
           VALUES (?, 'com.example.calendar', 'rev-1', 'activate', 'owner-1', ?, ?, '[]',
                   '2026-01-01T00:00:00Z', ?)"#,
    )
    .bind(approval_id)
    .bind(approved_by)
    .bind(status)
    .bind(resolved_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_legacy_revision(
    pool: &SqlitePool,
    revision_id: &str,
    package_id: &str,
    descriptor_json: &str,
    validation_json: &str,
    created_at: &str,
) {
    insert_legacy_revision_with_path(
        pool,
        revision_id,
        package_id,
        "managed/revision",
        descriptor_json,
        validation_json,
        created_at,
    )
    .await;
}

async fn insert_legacy_revision_with_path(
    pool: &SqlitePool,
    revision_id: &str,
    package_id: &str,
    storage_path: &str,
    descriptor_json: &str,
    validation_json: &str,
    created_at: &str,
) {
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at)
           VALUES (?, ?, '1.0.0', 'hash', ?, ?, ?, 'owner-1', ?)"#,
    )
    .bind(revision_id)
    .bind(package_id)
    .bind(storage_path)
    .bind(descriptor_json)
    .bind(validation_json)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_legacy_installation(
    pool: &SqlitePool,
    package_id: &str,
    active_revision_id: Option<&str>,
    enabled: i64,
    status: &str,
) {
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES (?, 'managed', ?, ?, 'approved', ?, ?, ?)"#,
    )
    .bind(package_id)
    .bind(active_revision_id)
    .bind(enabled)
    .bind(status)
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}
