use crate::skill_state::{SkillRevisionStatus, SkillStateStore};
use crate::storage::Storage;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use tempfile::TempDir;
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
    pool.close().await;

    assert!(Storage::connect(&url).await.is_err());
    assert_legacy_tables_intact(&url, 1, 1).await;
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

async fn insert_legacy_revision(
    pool: &SqlitePool,
    revision_id: &str,
    package_id: &str,
    descriptor_json: &str,
    validation_json: &str,
    created_at: &str,
) {
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at)
           VALUES (?, ?, '1.0.0', 'hash', 'managed/revision', ?, ?, 'owner-1', ?)"#,
    )
    .bind(revision_id)
    .bind(package_id)
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
